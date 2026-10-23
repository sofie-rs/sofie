#![cfg(feature = "uploads")]

use sofie::core::Error;
use sofie::db::Input as _;
use sofie::http::Upload;
use sofie::prelude::*;
use sofie::serde_json::json;
use sofie::values::{Control, FormFields};

#[derive(Input)]
pub struct NewPhoto {
    pub title: String,
    pub photo: Upload,
    pub caption: Option<Upload>,
}

#[test]
fn an_upload_field_is_a_file_control_and_cannot_come_from_json() {
    let controls: Vec<(&str, Control, bool)> = NewPhoto::FIELDS
        .iter()
        .map(|field| (field.name, field.input.control, field.required))
        .collect();

    assert_eq!(
        controls,
        [
            ("title", Control::Input, true),
            ("photo", Control::File, true),
            ("caption", Control::File, false),
        ]
    );
    let forged = json!({
        "title": "Forged",
        "photo": { "key": "0123456789abcdef0123456789abcdef", "filename": "a.png",
                   "content_type": "image/png", "size": 1, "seal": "AAAA" },
    });
    let Err(Error::Invalid(errors)) = NewPhoto::from_json(forged) else {
        panic!("a forged upload was read");
    };
    assert_eq!(errors[0].field, "photo");
}
