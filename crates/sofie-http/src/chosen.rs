use serde_json::{Value, json};
use sofie_core::Result;
use sofie_db::{Outcome, ReadOnly, Reply, ReplyData};

use crate::answer::Response;
use crate::routes::ScreenView;
use crate::screen;
use crate::view::RouteAnswer;

const CHOSEN_SCREEN: &str = ".screen";

/// `sofie check` holds the template against `T`; JSON callers get the data alone.
pub struct Screen<T> {
    name: &'static str,
    data: T,
}

impl<T> Screen<T> {
    pub fn render(name: &'static str, data: T) -> Self {
        Self { name, data }
    }
}

impl<T: Outcome> Outcome for Screen<T> {
    fn reply(self) -> Result<Reply> {
        let data = self.data.reply()?.into_json()?;
        Ok(Reply::Data(Box::new(Chosen(
            json!({ CHOSEN_SCREEN: self.name, "data": data }),
        ))))
    }
}

/// Told apart by its type, so no loader's own JSON, whatever its keys, chooses a screen.
struct Chosen(Value);

impl ReplyData for Chosen {
    fn write_json(&self, out: &mut Vec<u8>) -> serde_json::Result<()> {
        serde_json::to_writer(out, &self.0)
    }

    fn to_json(&self) -> serde_json::Result<Value> {
        Ok(self.0.clone())
    }
}

impl<T: ReadOnly> ReadOnly for Screen<T> {}

fn chosen(result: &Result<Reply>) -> Option<(String, Value)> {
    let Ok(Reply::Data(reply)) = result else {
        return None;
    };
    if reply.type_name() != std::any::type_name::<Chosen>() {
        return None;
    }
    let Ok(Value::Object(mut fields)) = reply.to_json() else {
        return None;
    };
    let name = fields.get(CHOSEN_SCREEN)?.as_str()?.to_owned();
    Some((name, fields.remove("data")?))
}

pub(crate) fn answer(
    route: &RouteAnswer<'_>,
    result: Result<Reply>,
) -> std::result::Result<Response, Result<Reply>> {
    let Some((name, data)) = chosen(&result) else {
        return Err(result);
    };
    let target = route.target;
    let view = ScreenView {
        #[cfg(feature = "resources")]
        builtin: None,
        key: crate::screen_key(target.module, target.audience.name, &name),
        title: sofie_engine::humanize(&name),
    };
    let caller = route.asked.caller;
    Ok(route.on_screen(&view, |page| {
        screen::answer(Ok(Reply::Json(data)), caller, page)
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_handler_s_screen_chooses_one_not_json_shaped_like_it() {
        let shaped = Ok(Reply::Json(json!({ CHOSEN_SCREEN: "orders", "data": [] })));
        let screen = Screen::render("orders", ()).reply();

        assert!(chosen(&shaped).is_none());
        assert_eq!(chosen(&screen), Some(("orders".to_owned(), Value::Null)));
    }
}
