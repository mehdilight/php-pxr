//! The `pxr` command line interface.
//!
//! ```text
//!   pxr check   main.php             type-check only, no code generation
//!   pxr build   main.php -o main     compile to a native executable
//!   pxr run     main.php             compile (in a temp dir) and run
//!   pxr emit-ir main.php             print the optimized PXR IR
//!   pxr emit-llvm main.php           print the generated LLVM IR
//!   pxr tokens  main.php             dump the token stream
//!   pxr ast     main.php             dump the parsed AST
//! ```

use pxr_diag::Diagnostics;
use pxr_driver::{BuildError, BuildOptions};
use std::path::{Path, PathBuf};
use std::process::Command;

const VERSION: &str = env!("CARGO_PKG_VERSION");

fn main() {
    std::process::exit(run());
}

const USAGE: &str = "\
PXR — PHP Extended Runtime compiler

USAGE:
    pxr <command> [options] <file.php> [-- program-args...]

COMMANDS:
    build <file.php> [-o <out>]   Compile to a native executable
    run   <file.php>              Compile to a temporary executable and run it
    check <file.php>              Type-check without generating code
    emit-ir <file.php>            Print the optimized PXR IR
    emit-llvm <file.php>          Print the generated LLVM IR
    tokens <file.php>             Print the lexer token stream
    ast <file.php>                Print the parsed abstract syntax tree
    version                       Print version information
    help                          Print this message

OPTIONS:
    -o, --output <path>   Output path for `build` (default: input stem)
        --opt <0-3>       Optimization level (default: 2)
        --keep-llvm       Keep the generated .ll next to the output
        --keep-temps      Keep intermediate files
    -v, --verbose         Print stage timings and statistics
    -h, --help            Print this message

EXAMPLES:
    pxr run examples/hello.php
    pxr build examples/fib.php -o fib && ./fib
    pxr check main.php
";

fn run() -> i32 {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        print!("{USAGE}");
        return 2;
    }

    let cmd = args[0].as_str();
    let rest = &args[1..];

    match cmd {
        "-h" | "--help" | "help" => {
            print!("{USAGE}");
            0
        }
        "-V" | "--version" | "version" => {
            println!("pxr {VERSION}");
            println!("PHP Extended Runtime compiler");
            0
        }
        "check" => cmd_check(rest),
        "build" => cmd_build(rest),
        "run" => cmd_run(rest),
        "emit-ir" => cmd_emit_ir(rest),
        "emit-llvm" => cmd_emit_llvm(rest),
        "tokens" => cmd_tokens(rest),
        "ast" => cmd_ast(rest),
        other => {
            eprintln!("pxr: unknown command `{other}`\n");
            eprint!("{USAGE}");
            2
        }
    }
}

#[derive(Default)]
struct Options {
    file: Option<PathBuf>,
    output: Option<PathBuf>,
    opt_level: u8,
    keep_llvm: bool,
    keep_temps: bool,
    verbose: bool,
    program_args: Vec<String>,
}

fn parse_options(args: &[String], allow_program_args: bool) -> Result<Options, String> {
    let mut o = Options {
        opt_level: 2,
        ..Default::default()
    };
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        match a.as_str() {
            "-o" | "--output" => {
                i += 1;
                let v = args.get(i).ok_or("`-o` requires a path")?;
                o.output = Some(PathBuf::from(v));
            }
            "--opt" => {
                i += 1;
                let v = args.get(i).ok_or("`--opt` requires a level 0-3")?;
                o.opt_level = v.parse().map_err(|_| format!("invalid --opt `{v}`"))?;
                if o.opt_level > 3 {
                    return Err("--opt must be between 0 and 3".into());
                }
            }
            "--keep-llvm" => o.keep_llvm = true,
            "--keep-temps" => o.keep_temps = true,
            "-v" | "--verbose" => o.verbose = true,
            "-h" | "--help" => return Err(String::new()),
            "--" if allow_program_args => {
                o.program_args.extend_from_slice(&args[i + 1..]);
                break;
            }
            other if other.starts_with('-') && other.len() > 1 => {
                return Err(format!("unknown option `{other}`"));
            }
            other => {
                if o.file.is_some() {
                    if allow_program_args {
                        // Extra positional arguments belong to the program.
                        o.program_args.extend_from_slice(&args[i..]);
                        break;
                    }
                    return Err(format!("unexpected argument `{other}`"));
                }
                o.file = Some(PathBuf::from(other));
            }
        }
        i += 1;
    }
    Ok(o)
}

fn cmd_check(args: &[String]) -> i32 {
    let o = match parse_options(args, false) {
        Ok(o) => o,
        Err(_) => {
            eprint!("{USAGE}");
            return 2;
        }
    };
    let Some(file) = o.file else {
        eprintln!("pxr check: missing input file");
        return 2;
    };
    let src = match read(&file) {
        Ok(s) => s,
        Err(code) => return code,
    };
    let (_, diags) = pxr_driver::check_path(&file);
    report(&diags, &src, &file)
}

fn cmd_build(args: &[String]) -> i32 {
    let o = match parse_options(args, false) {
        Ok(o) => o,
        Err(e) => {
            if !e.is_empty() {
                eprintln!("pxr build: {e}");
            }
            eprint!("{USAGE}");
            return 2;
        }
    };
    let Some(file) = o.file.clone() else {
        eprintln!("pxr build: missing input file");
        return 2;
    };
    let output = o
        .output
        .clone()
        .unwrap_or_else(|| PathBuf::from(file.file_stem().unwrap_or_default()));

    let opts = BuildOptions {
        input: file.clone(),
        output: output.clone(),
        opt_level: o.opt_level,
        keep_llvm: o.keep_llvm,
    };
    match pxr_driver::build(&opts) {
        Ok(result) => {
            if o.verbose {
                print_build_stats(&result, &file);
            }
            println!("compiled {} -> {}", file.display(), output.display());
            0
        }
        Err(e) => report_build_error(e, &file),
    }
}

fn cmd_run(args: &[String]) -> i32 {
    let o = match parse_options(args, true) {
        Ok(o) => o,
        Err(e) => {
            if !e.is_empty() {
                eprintln!("pxr run: {e}");
            }
            eprint!("{USAGE}");
            return 2;
        }
    };
    let Some(file) = o.file.clone() else {
        eprintln!("pxr run: missing input file");
        return 2;
    };
    if o.verbose {
        eprintln!("pxr: building {}", file.display());
    }
    let (exe, _workdir) = match pxr_driver::build_temp(&file, o.opt_level) {
        Ok(v) => v,
        Err(e) => return report_build_error(e, &file),
    };

    match Command::new(&exe).args(&o.program_args).status() {
        Ok(status) => status.code().unwrap_or(1),
        Err(e) => {
            eprintln!("pxr: failed to execute `{}`: {e}", exe.display());
            1
        }
    }
}

fn cmd_emit_ir(args: &[String]) -> i32 {
    with_source(args, |src, file| match pxr_driver::ir_for_file(file) {
        Ok((text, stats)) => {
            eprint!(
                "; pxr: {} constant(s) folded, {} branch(es) folded, {} instruction(s) removed\n",
                stats.constants_folded, stats.branches_folded, stats.instructions_removed
            );
            print!("{text}");
            0
        }
        Err(e) => report(&e.diagnostics, src, file),
    })
}

fn cmd_emit_llvm(args: &[String]) -> i32 {
    with_source(args, |src, file| {
        let o = parse_options(args, false);
        let _ = o;
        match pxr_driver::check_path(file) {
            (Some(program), diags) if !diags.has_errors() => {
                let mut module = pxr_codegen::lower_program(&program);
                pxr_opt::optimize(&mut module);
                let ll = pxr_codegen::emit_llvm(
                    &module,
                    &pxr_codegen::LlvmOptions {
                        source_name: file
                            .file_name()
                            .map(|s| s.to_string_lossy().into_owned())
                            .unwrap_or_default(),
                        triple: None,
                    },
                );
                print!("{ll}");
                0
            }
            (_, diags) => report(&diags, src, file),
        }
    })
}

fn cmd_tokens(args: &[String]) -> i32 {
    with_source(args, |src, _file| {
        let (tokens, diags) = pxr_lexer::lex(src);
        for t in &tokens {
            println!(
                "{:>4}..{:<4} {}",
                t.span.start,
                t.span.end,
                describe_token(&t.kind)
            );
        }
        if diags.has_errors() {
            report(&diags, src, Path::new("<input>"))
        } else {
            0
        }
    })
}

fn cmd_ast(args: &[String]) -> i32 {
    with_source(args, |src, _file| {
        let (ast, diags) = pxr_parser::parse(src);
        if diags.has_errors() {
            return report(&diags, src, Path::new("<input>"));
        }
        match ast {
            Some(p) => {
                println!("{p:#?}");
                0
            }
            None => 1,
        }
    })
}

/// Shared helper for read-only commands.
fn with_source(args: &[String], f: impl FnOnce(&str, &Path) -> i32) -> i32 {
    let o = match parse_options(args, false) {
        Ok(o) => o,
        Err(_) => {
            eprint!("{USAGE}");
            return 2;
        }
    };
    let Some(file) = o.file else {
        eprintln!("pxr: missing input file");
        return 2;
    };
    let src = match read(&file) {
        Ok(s) => s,
        Err(code) => return code,
    };
    f(&src, &file)
}

fn read(path: &Path) -> Result<String, i32> {
    match std::fs::read_to_string(path) {
        Ok(s) => Ok(s),
        Err(e) => {
            eprintln!("pxr: cannot read `{}`: {e}", path.display());
            Err(1)
        }
    }
}

fn report(diags: &Diagnostics, src: &str, file: &Path) -> i32 {
    if diags.items().is_empty() {
        return 0;
    }
    eprint!("{}", diags.render(src, &file.display().to_string()));
    if diags.has_errors() {
        1
    } else {
        0
    }
}

fn report_build_error(e: BuildError, file: &Path) -> i32 {
    match e {
        BuildError::Compile(c) => {
            let src = std::fs::read_to_string(file).unwrap_or_default();
            eprint!(
                "{}",
                c.diagnostics.render(&src, &file.display().to_string())
            );
            1
        }
        other => {
            eprintln!("pxr: {other}");
            2
        }
    }
}

fn print_build_stats(result: &pxr_driver::BuildResult, file: &Path) {
    eprintln!("pxr: build statistics for {}", file.display());
    let t = &result.timings;
    eprintln!("  frontend : {:>8.2} ms", t.frontend.as_secs_f64() * 1e3);
    eprintln!("  optimize : {:>8.2} ms", t.optimize.as_secs_f64() * 1e3);
    eprintln!("  codegen  : {:>8.2} ms", t.codegen.as_secs_f64() * 1e3);
    eprintln!("  runtime  : {:>8.2} ms", t.runtime_cc.as_secs_f64() * 1e3);
    eprintln!("  link     : {:>8.2} ms", t.link.as_secs_f64() * 1e3);
    eprintln!("  total    : {:>8.2} ms", t.total().as_secs_f64() * 1e3);
    let s = &result.stats;
    eprintln!(
        "  optimized: {} folded, {} branch-folded, {} removed, {} jumps removed",
        s.constants_folded, s.branches_folded, s.instructions_removed, s.jumps_removed
    );
    eprintln!(
        "  output   : {} bytes",
        std::fs::metadata(&result.output)
            .map(|m| m.len())
            .unwrap_or(0)
    );
}

fn describe_token(k: &pxr_lexer::TokenKind) -> String {
    use pxr_lexer::TokenKind::*;
    match k {
        Ident(s) => format!("Ident({s})"),
        Variable(s) => format!("Variable(${s})"),
        Int(v) => format!("Int({v})"),
        Float(v) => format!("Float({v})"),
        Str(s) => format!("Str({s:?})"),
        other => other.symbol().to_string(),
    }
}
