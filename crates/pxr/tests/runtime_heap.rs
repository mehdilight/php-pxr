//! Exercise the real embedded runtime, including its private allocation state.
use std::process::Command;

#[test]
fn managed_heap_lifecycle_and_overflow() {
    let cc = std::env::var("CC").unwrap_or_else(|_| "clang".into());
    if !Command::new(&cc)
        .arg("--version")
        .output()
        .is_ok_and(|out| out.status.success())
    {
        eprintln!("skipping: no C compiler");
        return;
    }
    let dir = std::env::temp_dir().join(format!("pxr-heap-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let source = dir.join("heap.c");
    let executable = dir.join("heap-test");
    std::fs::write(
        &source,
        format!(
            "{}\n{}",
            pxr_driver::RUNTIME_SOURCE,
            include_str!("../../../runtime/tests/heap.c")
        ),
    )
    .unwrap();
    let mut command = Command::new(cc);
    command.args(["-std=c11", "-O2", "-g"]);
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
    for mode in [
        "normal",
        "capacity",
        "fatal",
        "format-fatal",
        "collection",
        "array-index",
        "array-cow",
        "string-append",
        "object-storage",
        "heap-index",
    ] {
        let out = Command::new(&executable).arg(mode).output().unwrap();
        let fatal = mode.ends_with("fatal");
        assert_eq!(
            out.status.code(),
            Some(if fatal { 255 } else { 0 }),
            "{mode}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(out.stdout, b"heap clean\n", "{mode}");
        if fatal {
            let stderr = String::from_utf8_lossy(&out.stderr);
            assert!(stderr.contains("Fatal error: Uncaught"));
            assert!(!stderr.contains("runtime error:"), "{mode}: {stderr}");
            assert!(!stderr.contains("Sanitizer"), "{mode}: {stderr}");
        } else {
            assert!(out.stderr.is_empty(), "{mode}: {:?}", out.stderr);
        }
    }
    for mode in [
        "alloc-overflow",
        "append-overflow",
        "string-overflow",
        "mul-overflow",
        "add-overflow",
        "repeat-overflow",
        "object-overflow",
        "object-payload-overflow",
        "array-overflow",
    ] {
        let out = Command::new(&executable).arg(mode).output().unwrap();
        assert!(!out.status.success(), "{mode}");
        assert_eq!(out.stderr, b"pxr: out of memory\n", "{mode}");
    }
    std::fs::remove_dir_all(dir).unwrap();
}
