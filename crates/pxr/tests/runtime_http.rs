//! HTTP framing, request roots and bounded teardown against the embedded runtime.
use std::process::Command;

#[test]
fn bounded_http_runtime() {
    let dir = std::env::temp_dir().join(format!("pxr-http-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let source = dir.join("http.c");
    std::fs::write(
        &source,
        format!(
            "{}\n{}",
            pxr_driver::RUNTIME_SOURCE,
            include_str!("../../../runtime/tests/http.c")
        ),
    )
    .unwrap();
    for optimization in ["-O0", "-O3"] {
        let executable = dir.join(format!("http-{optimization}"));
        let mut command = Command::new(std::env::var("CC").unwrap_or_else(|_| "clang".into()));
        command.args([
            "-std=c11",
            optimization,
            "-g",
            "-Werror=implicit-function-declaration",
        ]);
        if std::env::var_os("PXR_RUNTIME_SANITIZERS").is_some() {
            command.args(["-fsanitize=address,undefined", "-fno-omit-frame-pointer"]);
        }
        let compiled = command
            .arg(&source)
            .args(["-pthread", "-lm", "-o"])
            .arg(&executable)
            .output()
            .unwrap();
        assert!(
            compiled.status.success(),
            "{}",
            String::from_utf8_lossy(&compiled.stderr)
        );
        for stress in [false, true] {
            let mut run = Command::new(&executable);
            if stress {
                run.env("PXR_GC_STRESS", "1");
            }
            let out = run.output().unwrap();
            assert!(
                out.status.success(),
                "{optimization} stress={stress}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            assert_eq!(out.stdout, b"http clean\n");
            assert!(
                out.stderr.is_empty(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
    }
    std::fs::remove_dir_all(dir).unwrap();
}
