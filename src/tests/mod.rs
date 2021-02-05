mod guard;
mod lazy;
mod mutex;
mod poison;

#[test]
fn ui() {
    let t = trybuild::TestCases::new();
    t.pass("tests/ui/pass/*.rs");
}
