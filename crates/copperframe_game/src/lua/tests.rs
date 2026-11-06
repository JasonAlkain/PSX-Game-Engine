use super::*;

#[test]
fn errors_lose_the_file_prefix_and_keep_the_line() {
    let e = mlua::Error::runtime("scripts/a.lua:12: attempt to index a nil value");
    assert_eq!(
        describe(&e, "scripts/a.lua"),
        (Some(12), "attempt to index a nil value".into())
    );
    let e = mlua::Error::runtime("scripts/a.lua: the script must return a table");
    assert_eq!(
        describe(&e, "scripts/a.lua"),
        (None, "the script must return a table".into())
    );
}
