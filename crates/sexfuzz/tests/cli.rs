use std::process::Command;

fn output(arguments: &[&str]) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_sex-fuzz"))
        .args(arguments)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn direct_case_seed_replays_the_actual_first_case_in_each_suite() {
    // Independent SplitMix64 first output for root seed 0, not a second call
    // with that output as a new root seed.
    let first_seed = "16294208416658607535";
    for suite in ["dsp", "io", "all"] {
        let batch = output(&["--suite", suite, "--cases", "1", "--seed", "0"]);
        let replay = output(&["--suite", suite, "--case-seed", first_seed]);
        assert_eq!(
            batch.split("checksum ").nth(1),
            replay.split("checksum ").nth(1)
        );
        assert!(replay.contains(&format!("case seed {first_seed}")));
    }
}

#[test]
fn invalid_replay_combination_exits_before_running_a_case() {
    let output = Command::new(env!("CARGO_BIN_EXE_sex-fuzz"))
        .args(["--case-seed", "0", "--cases", "1"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("cannot be combined"));
}
