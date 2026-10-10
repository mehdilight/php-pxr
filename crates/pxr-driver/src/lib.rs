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
pub const RUNTIME_SOURCE: &str = include_str!("../../../runtime/pxr_rt.c");

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
    let src = read_source(path)?;
    let (_, diags) = check_source(&src);
    Ok(diags)
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
    let src = read_source(&opts.input).map_err(BuildError::Compile)?;

    let t0 = Instant::now();
    let (program, diags) = check_source(&src);
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

/// Compile the embedded runtime to a cached object file, reusing it when the
/// source and compiler are unchanged.
fn compile_runtime(cc: &str, workspace: &WorkDir) -> Result<PathBuf, BuildError> {
    let dir = cache_dir();
    std::fs::create_dir_all(&dir).map_err(BuildError::Io)?;
    let hash = fnv1a(RUNTIME_SOURCE.as_bytes());
    let obj = dir.join(format!("pxr_rt-{hash:016x}.o"));
    if obj.exists() {
        return Ok(obj);
    }

    let c_path = dir.join(format!("pxr_rt-{hash:016x}.c"));
    write_file(&c_path, RUNTIME_SOURCE).map_err(BuildError::Io)?;

    let out = run_tool(
        Command::new(cc)
            .arg("-c")
            .arg("-O2")
            .arg("-std=c11")
            .arg("-o")
            .arg(&obj)
            .arg(&c_path),
        "compiling the PXR runtime",
    );
    match out {
        Ok(()) => Ok(obj),
        Err(e) => {
            // Fall back to a private copy in the per-build workspace when the
            // cache directory is not writable.
            let obj = workspace.path.join("pxr_rt.o");
            run_tool(
                Command::new(cc)
                    .arg("-c")
                    .arg("-O2")
                    .arg("-std=c11")
                    .arg("-o")
                    .arg(&obj)
                    .arg(&c_path),
                "compiling the PXR runtime",
            )?;
            let _ = e;
            Ok(obj)
        }
    }
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
    s.lines()
        .map(|l| format!("    {l}\n"))
        .collect::<String>()
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
