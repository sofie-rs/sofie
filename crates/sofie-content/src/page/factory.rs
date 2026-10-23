use sofie::prelude::*;
use sofie::values::Url;

use super::{Markdown, Page};

#[derive(Input)]
pub struct PageInput {
    #[required]
    #[length(max = 200)]
    title: String,
    #[required]
    #[length(max = 500)]
    summary: String,
    /// Below the request's 64 KiB, so a long page is refused by name rather than by size.
    #[required]
    #[length(max = 50_000)]
    body: Markdown,
    image: Option<Url>,
}

impl sofie::db::Create for Page {
    type Input = PageInput;
    type Owner = ();

    fn create(input: PageInput, (): ()) -> Result<Self> {
        Ok(Self {
            meta: Meta::new()?,
            title: input.title,
            summary: input.summary,
            body: input.body,
            image: input.image,
            published_at: None,
        })
    }
}

impl sofie::db::Update for Page {
    type Input = PageInput;

    fn update(&mut self, input: PageInput) -> Result<()> {
        self.title = input.title;
        self.summary = input.summary;
        self.body = input.body;
        self.image = input.image;
        Ok(())
    }
}
