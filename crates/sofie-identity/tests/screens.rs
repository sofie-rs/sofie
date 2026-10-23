use sofie::http::{Area, Web};

#[test]
fn the_built_in_screens_load_and_take_their_forms_from_the_credentials() {
    let mut screens = sofie::http::Screens::default();
    screens
        .add_builtin(sofie_identity::module(&[]).screens)
        .unwrap();

    screens.describe_forms(sofie_identity::FORMS).unwrap();
}

#[test]
fn its_module_loads_every_screen_with_its_forms_and_migrates_its_tables() {
    let modules = [sofie_identity::module(&[])];

    sofie::http::wired_screens(&modules, &[]).unwrap();

    assert_eq!(sofie::http::migrations(&modules).len(), 1);
}

#[test]
fn each_built_in_screen_has_the_key_its_route_renders() {
    for (key, _) in sofie_identity::module(&[]).screens {
        let name = key.rsplit('/').next().unwrap();
        assert_eq!(
            *key,
            sofie::http::screen_key("sofie_identity", Web::NAME, name)
        );
    }
}
