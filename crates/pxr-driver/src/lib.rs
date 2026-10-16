//! The PXR compilation driver.
//!
//! This crate wires the compiler stages together and shells out to the system
//! C compiler (clang) to turn generated LLVM IR into a native executable:
//!
//! ```text
//!   .php source
//!     -> pxr-lexer       tokens
//!     -> pxr-parser      AST
//!     -> pxr-sema        typed AST + type errors
//!     -> pxr-codegen     IR (lowering)
//!     -> pxr-opt         optimized IR
//!     -> pxr-codegen     LLVM IR text
//!     -> clang           object code + link with the PXR runtime
//!     -> native executable
//! ```

use pxr_diag::{CompileError, Diagnostics};
use pxr_ir::Module;
use pxr_opt::OptStats;
use pxr_sema::TProgram;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

/// The runtime C source, embedded so the `pxr` binary is self contained.
pub const RUNTIME_SOURCE: &str = concat!(
    include_str!("../../../runtime/pxr_rt.c"),
    "\n",
    include_str!("../../../runtime/pxr_async.c")
);
const PHP_WORKER_SOURCE: &str = include_str!("../../../runtime/pxr_php_worker.php");

fn runtime_source() -> String {
    let mut source = String::from("const char pxr_php_worker_source[] =\n");
    for chunk in PHP_WORKER_SOURCE
        .trim_start_matches("<?php")
        .as_bytes()
        .chunks(96)
    {
        source.push('"');
        for byte in chunk {
            // Three-digit octal escapes cannot consume following source bytes.
            write!(source, "\\{byte:03o}").unwrap();
        }
        source.push_str("\"\n");
    }
    source.push_str(";\n");
    source.push_str(RUNTIME_SOURCE);
    source
}

#[derive(Clone, Debug)]
pub struct BuildOptions {
    pub input: PathBuf,
    pub output: PathBuf,
    /// Optimization level handed to clang (0..=3).
    pub opt_level: u8,
    /// Keep intermediate `.ll` files next to the output.
    pub keep_llvm: bool,
}

impl BuildOptions {
    pub fn new(input: impl Into<PathBuf>, output: impl Into<PathBuf>) -> Self {
        BuildOptions {
            input: input.into(),
            output: output.into(),
            opt_level: 2,
            keep_llvm: false,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Timings {
    pub frontend: Duration,
    pub optimize: Duration,
    pub codegen: Duration,
    pub runtime_cc: Duration,
    pub link: Duration,
}

impl Timings {
    pub fn total(&self) -> Duration {
        self.frontend + self.optimize + self.codegen + self.runtime_cc + self.link
    }
}

#[derive(Debug)]
pub struct Frontend {
    pub program: TProgram,
    /// Optimization statistics are filled in by [`build`].
    pub diagnostics: Diagnostics,
}

/// Run the front end (lex, parse, semantic analysis) over a source string.
pub fn check_source(src: &str) -> (Option<TProgram>, Diagnostics) {
    let (prog, mut diags) = pxr_parser::parse(src);
    let Some(prog) = prog else {
        return (None, diags);
    };
    if diags.has_errors() {
        return (None, diags);
    }
    let (tprog, sdiags) = pxr_sema::analyze(&prog);
    diags.extend(sdiags);
    if diags.has_errors() {
        (None, diags)
    } else {
        (tprog, diags)
    }
}

pub fn check_file(path: &Path) -> Result<Diagnostics, CompileError> {
    read_source(path)?;
    Ok(check_path(path).1)
}

/// Resolve declaration-only modules and check a complete file-based program.
pub fn check_path(path: &Path) -> (Option<TProgram>, Diagnostics) {
    let (program, diagnostics, _) = check_project(path);
    (program, diagnostics)
}

fn check_project(path: &Path) -> (Option<TProgram>, Diagnostics, Vec<MigrationFile>) {
    let mut loader = ModuleLoader::default();
    loader.load(path, false, true, true, pxr_diag::Span::synthetic());
    let mut program = None;
    if !loader.diags.has_errors() {
        let ast = pxr_ast::Program {
            items: std::mem::take(&mut loader.items),
            span: pxr_diag::Span::synthetic(),
        };
        let (typed, diags) = pxr_sema::analyze(&ast);
        loader.diags.extend(diags);
        program = typed;
    }
    let mut diagnostics = Diagnostics::new();
    for original in loader.diags.items() {
        let mut diag = original.clone();
        if let Some(span) = &mut diag.span {
            if !span.is_synthetic() {
                if let Some((base, path, source)) =
                    loader.sources.iter().find(|(base, _, source)| {
                        span.start >= *base && span.start <= base + source.len()
                    })
                {
                    span.start -= base;
                    span.end = span.end.saturating_sub(*base).min(source.len());
                    diag.source = Some((path.display().to_string(), source.clone()));
                }
            }
        }
        diagnostics.emit(diag);
    }
    if diagnostics.has_errors() {
        program = None;
    }
    let mut files = std::collections::BTreeMap::new();
    for (_, path, source) in &loader.sources {
        files.entry(path.clone()).or_insert_with(|| MigrationFile {
            path: path.clone(),
            dynamic: dynamic_source(source),
            lines: source.lines().count(),
        });
    }
    (program, diagnostics, files.into_values().collect())
}

/// A file's migration boundary; dynamic files are checked by PHP at runtime.
#[derive(Clone, Debug)]
pub struct MigrationFile {
    pub path: PathBuf,
    pub dynamic: bool,
    pub lines: usize,
}

#[derive(Clone, Debug)]
pub struct MigrationFunction {
    pub name: String,
    pub mixed_slots: usize,
    pub concrete_slots: usize,
    pub mixed_return: bool,
    pub dynamic_calls: usize,
}

#[derive(Clone, Debug)]
pub struct MigrationReport {
    pub checked: bool,
    pub files: Vec<MigrationFile>,
    pub functions: Vec<MigrationFunction>,
}

fn has_mixed(ty: &pxr_types::Ty) -> bool {
    match ty {
        pxr_types::Ty::Mixed => true,
        pxr_types::Ty::Array(element) => has_mixed(element),
        _ => false,
    }
}

/// Inspect the same graph and semantic analysis used by build/check. It never
/// executes a dynamic file or runs PHP to discover exports.
pub fn migration_report(path: &Path) -> (MigrationReport, Diagnostics) {
    let (program, diagnostics, files) = check_project(path);
    let mut report = MigrationReport {
        checked: program.is_some() && !diagnostics.has_errors(),
        files,
        functions: Vec::new(),
    };
    if let Some(program) = program {
        let module = pxr_codegen::lower_program(&program);
        for function in program.funcs.iter().chain(std::iter::once(&program.main)) {
            let mixed_slots = function.slots.iter().filter(|ty| has_mixed(ty)).count();
            let dynamic_calls = module.functions.iter().find(|f| f.name == function.name)
                .map(|f| f.body.iter().filter(|i| matches!(i, pxr_ir::Inst::Call { callee, .. } if callee == "pxr_php_call")).count()).unwrap_or(0);
            report.functions.push(MigrationFunction {
                name: function.display.clone(),
                mixed_slots,
                concrete_slots: function.slots.len() - mixed_slots,
                mixed_return: has_mixed(&function.ret),
                dynamic_calls,
            });
        }
        report.functions.sort_by(|a, b| a.name.cmp(&b.name));
    }
    (report, diagnostics)
}

fn json_string(text: &str) -> String {
    let mut result = String::from("\"");
    for ch in text.chars() {
        match ch {
            '"' => result.push_str("\\\""),
            '\\' => result.push_str("\\\\"),
            '\n' => result.push_str("\\n"),
            '\r' => result.push_str("\\r"),
            '\t' => result.push_str("\\t"),
            ch if ch < ' ' => {
                write!(result, "\\u{:04x}", ch as u32).unwrap();
            }
            ch => result.push(ch),
        }
    }
    result.push('"');
    result
}

impl MigrationReport {
    pub fn to_json(&self) -> String {
        let files = self
            .files
            .iter()
            .map(|file| {
                format!(
                    "{{\"path\":{},\"mode\":\"{}\",\"lines\":{}}}",
                    json_string(&file.path.to_string_lossy()),
                    if file.dynamic { "php" } else { "native" },
                    file.lines
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        let functions = self.functions.iter().map(|function| format!("{{\"name\":{},\"mixed_slots\":{},\"concrete_slots\":{},\"mixed_return\":{},\"dynamic_calls\":{}}}",
            json_string(&function.name), function.mixed_slots, function.concrete_slots, function.mixed_return, function.dynamic_calls)).collect::<Vec<_>>().join(",");
        format!(
            "{{\"schema_version\":1,\"checked\":{},\"files\":[{}],\"functions\":[{}]}}",
            self.checked, files, functions
        )
    }
    pub fn to_text(&self) -> String {
        let mut text = format!(
            "Migration report: {}\n",
            if self.checked {
                "native code checks passed"
            } else {
                "native code has errors"
            }
        );
        for file in &self.files {
            writeln!(
                text,
                "  {}  {} ({} lines)",
                if file.dynamic { "PHP   " } else { "native" },
                file.path.display(),
                file.lines
            )
            .unwrap();
        }
        for function in &self.functions {
            writeln!(
                text,
                "  {}: {} concrete slots, {} mixed slots, {} dynamic calls{}",
                function.name,
                function.concrete_slots,
                function.mixed_slots,
                function.dynamic_calls,
                if function.mixed_return {
                    ", mixed return"
                } else {
                    ""
                }
            )
            .unwrap();
        }
        text.push_str("PHP boundaries are not statically checked; their runtime dependencies are resolved by PHP.\n");
        text
    }
}

fn dynamic_source(source: &str) -> bool {
    let mut rest = source.trim_start();
    if let Some(source) = rest.strip_prefix("<?php") {
        rest = source;
    }
    loop {
        rest = rest.trim_start();
        if rest.starts_with("//") || rest.starts_with('#') {
            let (line, tail) = rest.split_once('\n').unwrap_or((rest, ""));
            if line.trim_end() == "// @pxr-dynamic" {
                return true;
            }
            rest = tail;
        } else if rest.starts_with("/*") {
            let Some((_, tail)) = rest.split_once("*/") else {
                return false;
            };
            rest = tail;
        } else {
            return false;
        }
    }
}

#[derive(Default)]
struct ModuleLoader {
    items: Vec<pxr_ast::Stmt>,
    sources: Vec<(usize, PathBuf, String)>,
    next_offset: usize,
    seen: std::collections::HashSet<PathBuf>,
    active: std::collections::HashSet<PathBuf>,
    diags: Diagnostics,
}

impl ModuleLoader {
    fn error(&mut self, span: pxr_diag::Span, message: impl Into<String>) {
        self.diags.emit(
            pxr_diag::Diagnostic::error(message)
                .with_code("E0273")
                .with_span(span),
        );
    }

    fn resolve_runtime_include(&mut self, statement: &mut pxr_ast::Stmt, file: &Path) {
        if let pxr_ast::Stmt::Require {
            path,
            once,
            required,
            dir_relative,
            span,
        } = statement
        {
            let directory = file.parent().unwrap();
            let dependency = if *dir_relative {
                PathBuf::from(format!("{}{path}", directory.display()))
            } else {
                directory.join(path)
            };
            if dependency.as_os_str().as_encoded_bytes().contains(&0) {
                self.error(*span, "module paths cannot contain NUL bytes");
                return;
            }
            let dynamic = std::fs::read_to_string(&dependency).is_ok_and(|s| dynamic_source(&s));
            if dynamic || (!*required && !dependency.exists()) {
                let before = self.items.len();
                self.load(&dependency, *once, *required, false, *span);
                if self.items.len() == before + 1 {
                    *statement = self.items.pop().unwrap();
                }
            }
            return;
        }
        for body in statement.bodies_mut() {
            for statement in body {
                self.resolve_runtime_include(statement, file);
            }
        }
    }

    fn load(
        &mut self,
        requested: &Path,
        once: bool,
        required: bool,
        entry: bool,
        site: pxr_diag::Span,
    ) {
        let path = match requested.canonicalize() {
            Ok(path) => path,
            Err(_) if !required => {
                let path = if requested.is_absolute() {
                    requested.to_path_buf()
                } else {
                    std::env::current_dir().unwrap_or_default().join(requested)
                };
                self.items.push(pxr_ast::Stmt::PhpInclude {
                    path: path.to_string_lossy().into_owned(),
                    mode: u8::from(once),
                    span: site,
                });
                return;
            }
            Err(error) => {
                self.error(
                    site,
                    format!("cannot resolve module `{}`: {error}", requested.display()),
                );
                return;
            }
        };
        let source = match read_source(&path) {
            Ok(source) => source,
            Err(_) => {
                self.error(site, format!("cannot read module `{}`", path.display()));
                return;
            }
        };
        let offset = self.next_offset + 1;
        self.next_offset = offset + source.len();
        if dynamic_source(&source) {
            self.sources.push((offset, path.clone(), source));
            self.items.push(pxr_ast::Stmt::PhpInclude {
                path: path.to_string_lossy().into_owned(),
                mode: u8::from(once) + if required { 2 } else { 0 },
                span: site,
            });
            return;
        }
        if once && self.seen.contains(&path) {
            return;
        }
        if self.active.contains(&path) || self.active.len() >= 128 {
            self.error(site, format!("cyclic or excessively deep module dependency at `{}`; use require_once for shared dependencies", path.display()));
            return;
        }
        let (program, diagnostics) = pxr_parser::parse_at(&source, offset);
        self.sources.push((offset, path.clone(), source));
        let failed = diagnostics.has_errors();
        self.diags.extend(diagnostics);
        if failed {
            return;
        }
        self.seen.insert(path.clone());
        self.active.insert(path.clone());
        let mut executable = false;
        if let Some(program) = program {
            for mut item in program.items {
                for body in item.bodies_mut() {
                    for statement in body {
                        self.resolve_runtime_include(statement, &path);
                    }
                }
                match item {
                    pxr_ast::Stmt::Require {
                        path: dependency,
                        once,
                        required,
                        dir_relative,
                        span,
                    } => {
                        let directory = path.parent().unwrap();
                        let dependency = if dir_relative {
                            PathBuf::from(format!("{}{dependency}", directory.display()))
                        } else {
                            directory.join(dependency)
                        };
                        let dynamic =
                            std::fs::read_to_string(&dependency).is_ok_and(|s| dynamic_source(&s));
                        if executable && !dynamic && (required || dependency.exists()) {
                            self.error(
                                span,
                                "module dependencies must precede executable top-level statements",
                            );
                            continue;
                        }
                        if dependency.as_os_str().as_encoded_bytes().contains(&0) {
                            self.error(span, "module paths cannot contain NUL bytes");
                            continue;
                        }
                        self.load(&dependency, once, required, false, span);
                    }
                    pxr_ast::Stmt::FnDecl(_)
                    | pxr_ast::Stmt::ClassDecl(_)
                    | pxr_ast::Stmt::Empty => self.items.push(item),
                    item if !entry => self.error(
                        item.span(),
                        format!(
                            "module `{}` must contain declarations and dependencies only",
                            path.display()
                        ),
                    ),
                    _ => {
                        executable = true;
                        self.items.push(item);
                    }
                }
            }
        }
        self.active.remove(&path);
    }
}

/// Result of a successful build.
#[derive(Debug)]
pub struct BuildResult {
    pub output: PathBuf,
    pub llvm_ir: String,
    pub ir_text: String,
    pub stats: OptStats,
    pub timings: Timings,
}

pub fn build(opts: &BuildOptions) -> Result<BuildResult, BuildError> {
    let t0 = Instant::now();
    let (program, diags) = check_path(&opts.input);
    let frontend_time = t0.elapsed();
    if diags.has_errors() || program.is_none() {
        return Err(BuildError::Compile(CompileError::new(diags)));
    }
    let program = program.unwrap();

    let mut timings = Timings {
        frontend: frontend_time,
        ..Default::default()
    };

    // Lower -> optimize -> emit LLVM IR.
    let t1 = Instant::now();
    let mut module = pxr_codegen::lower_program(&program);
    let stats = pxr_opt::optimize(&mut module);
    timings.optimize = t1.elapsed();

    let ir_text = module.to_text();

    let t2 = Instant::now();
    let llvm_ir = pxr_codegen::emit_llvm(
        &module,
        &pxr_codegen::LlvmOptions {
            source_name: opts
                .input
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default(),
            triple: None,
        },
    );
    timings.codegen = t2.elapsed();

    // Compile (or reuse) the runtime object, then link.
    let cc = find_cc()?;
    let workspace = WorkDir::create()?;

    let t3 = Instant::now();
    let runtime_obj = compile_runtime(&cc, &workspace)?;
    timings.runtime_cc = t3.elapsed();

    let ll_path = workspace.path.join(format!(
        "{}.ll",
        opts.input
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "module".into())
    ));
    write_file(&ll_path, &llvm_ir).map_err(BuildError::Io)?;

    let t4 = Instant::now();
    link(&cc, &ll_path, &runtime_obj, &opts.output, opts.opt_level)?;
    timings.link = t4.elapsed();

    if opts.keep_llvm {
        let keep = opts.output.with_extension("ll");
        write_file(&keep, &llvm_ir).map_err(BuildError::Io)?;
    }

    Ok(BuildResult {
        output: opts.output.clone(),
        llvm_ir,
        ir_text,
        stats,
        timings,
    })
}

/// Build into a temporary directory and return its path (used by `pxr run`).
pub fn build_temp(src_path: &Path, opt_level: u8) -> Result<(PathBuf, WorkDir), BuildError> {
    let workspace = WorkDir::create()?;
    let stem = src_path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "program".into());
    let out = workspace.path.join(stem);
    let opts = BuildOptions {
        input: src_path.to_path_buf(),
        output: out.clone(),
        opt_level,
        keep_llvm: false,
    };
    let result = build(&opts)?;
    Ok((result.output, workspace))
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum BuildError {
    Compile(CompileError),
    Io(std::io::Error),
    Toolchain(String),
}

impl std::fmt::Display for BuildError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BuildError::Compile(e) => write!(f, "{e}"),
            BuildError::Io(e) => write!(f, "io error: {e}"),
            BuildError::Toolchain(m) => write!(f, "toolchain error: {m}"),
        }
    }
}

impl std::error::Error for BuildError {}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn read_source(path: &Path) -> Result<String, CompileError> {
    std::fs::read_to_string(path).map_err(|e| {
        let mut d = Diagnostics::new();
        d.emit(pxr_diag::Diagnostic::error(format!(
            "cannot read `{}`: {e}",
            path.display()
        )));
        CompileError::new(d)
    })
}

fn write_file(path: &Path, contents: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, contents)
}

fn find_cc() -> Result<String, BuildError> {
    if let Ok(cc) = std::env::var("CC") {
        if !cc.is_empty() {
            return Ok(cc);
        }
    }
    Ok("clang".to_string())
}

/// A scratch directory that is removed when dropped.
pub struct WorkDir {
    pub path: PathBuf,
    /// When false the directory is left behind (for `--keep-temps`).
    cleanup: bool,
}

impl WorkDir {
    pub fn create() -> Result<Self, BuildError> {
        let base = std::env::temp_dir().join(format!("pxr-{}", std::process::id()));
        std::fs::create_dir_all(&base).map_err(BuildError::Io)?;
        let unique = base.join(format!(
            "{}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
            rand_suffix()
        ));
        std::fs::create_dir_all(&unique).map_err(BuildError::Io)?;
        Ok(WorkDir {
            path: unique,
            cleanup: true,
        })
    }

    pub fn keep(mut self) -> PathBuf {
        self.cleanup = false;
        self.path.clone()
    }
}

impl Drop for WorkDir {
    fn drop(&mut self) {
        if self.cleanup {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}

fn rand_suffix() -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
        .hash(&mut h);
    std::process::id().hash(&mut h);
    h.finish()
}

fn cache_dir() -> PathBuf {
    if let Ok(d) = std::env::var("PXR_CACHE_DIR") {
        return PathBuf::from(d);
    }
    if let Ok(h) = std::env::var("HOME") {
        return PathBuf::from(h).join(".cache").join("pxr");
    }
    std::env::temp_dir().join("pxr-cache")
}

/// Compile privately and publish a complete cache object atomically. Parallel
/// builds must never link a partially written object or compile a partial source.
fn compile_runtime(cc: &str, workspace: &WorkDir) -> Result<PathBuf, BuildError> {
    let dir = cache_dir();
    let source = runtime_source();
    let hash = fnv1a(format!("{cc}\n{source}").as_bytes());
    let cached = dir.join(format!("pxr_rt-{hash:016x}.o"));
    if cached.is_file() {
        return Ok(cached);
    }

    let c_path = workspace.path.join("pxr_rt.c");
    let object = workspace.path.join("pxr_rt.o");
    write_file(&c_path, &source).map_err(BuildError::Io)?;
    run_tool(
        Command::new(cc)
            .arg("-c")
            .arg("-O2")
            .arg("-std=c11")
            .arg("-o")
            .arg(&object)
            .arg(&c_path),
        "compiling the PXR runtime",
    )?;

    // A failed/unwritable cache is optional; the private object still works.
    if std::fs::create_dir_all(&dir).is_ok() {
        let staged = dir.join(format!(
            "pxr_rt-{hash:016x}-{}-{}.tmp",
            std::process::id(),
            rand_suffix()
        ));
        if std::fs::copy(&object, &staged).is_ok() && std::fs::rename(&staged, &cached).is_ok() {
            return Ok(cached);
        }
        let _ = std::fs::remove_file(staged);
    }
    Ok(object)
}

fn link(
    cc: &str,
    ll_path: &Path,
    runtime_obj: &Path,
    output: &Path,
    opt_level: u8,
) -> Result<(), BuildError> {
    if let Some(parent) = output.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(BuildError::Io)?;
        }
    }
    let mut cmd = Command::new(cc);
    cmd.arg(format!("-O{opt_level}"))
        .arg(ll_path)
        .arg(runtime_obj)
        .arg("-o")
        .arg(output)
        .arg("-Wno-override-module");
    run_tool(&mut cmd, "linking")?;
    Ok(())
}

fn run_tool(cmd: &mut Command, what: &str) -> Result<(), BuildError> {
    let display = format!(
        "{} {}",
        cmd.get_program().to_string_lossy(),
        cmd.get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(" ")
    );
    let out = cmd.output().map_err(|e| {
        BuildError::Toolchain(format!(
            "{what}: could not run `{display}`: {e}\n\
             hint: PXR needs a C compiler (clang) on PATH; set $CC to override"
        ))
    })?;
    if !out.status.success() {
        let mut msg = String::new();
        let _ = write!(msg, "{what} failed\n  command: {display}\n");
        let stderr = String::from_utf8_lossy(&out.stderr);
        if !stderr.trim().is_empty() {
            let _ = write!(msg, "  stderr:\n{}", indent(&stderr));
        }
        return Err(BuildError::Toolchain(msg));
    }
    Ok(())
}

fn indent(s: &str) -> String {
    s.lines().map(|l| format!("    {l}\n")).collect::<String>()
}

/// FNV-1a 64-bit hash, used only for cache keys.
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// The IR text for a source string (used by `pxr emit-ir`).
pub fn ir_for_source(src: &str) -> Result<(String, OptStats), CompileError> {
    let (program, diags) = check_source(src);
    if diags.has_errors() || program.is_none() {
        return Err(CompileError::new(diags));
    }
    let mut module: Module = pxr_codegen::lower_program(&program.unwrap());
    let stats = pxr_opt::optimize(&mut module);
    Ok((module.to_text(), stats))
}

/// Optimized IR for a complete module graph.
pub fn ir_for_file(path: &Path) -> Result<(String, OptStats), CompileError> {
    let (program, diagnostics) = check_path(path);
    let Some(program) = program else {
        return Err(CompileError::new(diagnostics));
    };
    let mut module = pxr_codegen::lower_program(&program);
    let stats = pxr_opt::optimize(&mut module);
    Ok((module.to_text(), stats))
}
