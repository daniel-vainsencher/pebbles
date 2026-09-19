// r[verify no-jj-skip]
//
// Every trycmd fixture under tests/cmd/ and tests/workflows/ ships a `.no_jj`
// marker. If the skip logic regressed, these tests would either fail to
// initialize (no jj repo available in the trycmd tempdir) or leak commits
// into the trycmd output assertions. So the whole suite functioning is a
// live verification that .no_jj suppresses jj invocation.
#[test]
fn cli_tests() {
    // Forced-color environment variables make the binary colorize its output
    // even when piped, which breaks trycmd snapshot comparisons with confusing
    // diffs. Do not silently scrub the environment; warn loudly instead.
    for var in ["CLICOLOR_FORCE", "FORCE_COLOR"] {
        if std::env::var(var).map(|v| v != "0").unwrap_or(false) {
            eprintln!(
                "WARNING: {} is set in this environment. Forced color output \
                 interferes with trycmd snapshot tests and will make them fail \
                 with ANSI-escape diffs. Unset it (e.g. `env -u {} cargo test`) \
                 before debugging snapshot mismatches.",
                var, var
            );
        }
    }

    trycmd::TestCases::new()
        .case("tests/cmd/*.trycmd")
        .case("tests/cmd/*.toml")
        .case("tests/workflows/*.trycmd");
}
