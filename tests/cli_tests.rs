// r[verify no-jj-skip]
//
// Every trycmd fixture under tests/cmd/ and tests/workflows/ ships a `.no_jj`
// marker. If the skip logic regressed, these tests would either fail to
// initialize (no jj repo available in the trycmd tempdir) or leak commits
// into the trycmd output assertions. So the whole suite functioning is a
// live verification that .no_jj suppresses jj invocation.
#[test]
fn cli_tests() {
    trycmd::TestCases::new()
        .case("tests/cmd/*.trycmd")
        .case("tests/cmd/*.toml")
        .case("tests/workflows/*.trycmd");
}
