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

#[test]
fn entity_handles_round_trip_through_lua_integers() {
    let mut world = hecs::World::new();
    let a = world.spawn(());
    world.despawn(a).unwrap();
    let b = world.spawn(()); // same index, newer generation
    for e in [a, b] {
        assert_eq!(entity(to_lua(e)).unwrap(), e);
    }
    assert!(entity(0).is_err());
}

#[test]
fn angles_are_degrees_yaw_pitch_roll() {
    let q = from_degrees(90.0, 10.0, -5.0);
    let (y, p, r) = degrees(q);
    assert!((y - 90.0).abs() < 1e-3 && (p - 10.0).abs() < 1e-3 && (r + 5.0).abs() < 1e-3);
}
