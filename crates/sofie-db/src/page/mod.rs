#[cfg(feature = "listing")]
mod listing;

use serde::ser::SerializeStruct;
use serde::{Serialize, Serializer};
use sofie_core::listing::{HAS_NEXT, ITEMS, PAGE, SIZE};
use sofie_core::show::Fields;
use sofie_core::{Error, Result, Show, Shown};

#[cfg(feature = "listing")]
pub use self::listing::*;
use crate::{Outcome, Query, Read, ReadOnly, Reply, Showing, Source};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Page {
    number: u64,
    size: u64,
}

impl Page {
    /// Larger pages are refused, so a request cannot read a whole table.
    pub const MAX_SIZE: u64 = 100;
    /// A database reads and drops every row before the offset.
    pub const MAX_OFFSET: u64 = 10_000;

    pub fn new(number: u64, size: u64) -> Result<Self> {
        if number == 0 {
            return Err(Error::BadRequest("pages are numbered from 1".to_owned()));
        }
        if size == 0 || size > Self::MAX_SIZE {
            return Err(Error::BadRequest(format!(
                "a page holds 1 to {} records, not {size}",
                Self::MAX_SIZE
            )));
        }
        if (number - 1)
            .checked_mul(size)
            .is_none_or(|offset| offset > Self::MAX_OFFSET)
        {
            return Err(Error::BadRequest(format!(
                "page {number} starts past the first {} records; narrow the filter",
                Self::MAX_OFFSET
            )));
        }
        Ok(Self { number, size })
    }

    #[must_use]
    pub fn number(self) -> u64 {
        self.number
    }

    #[must_use]
    pub fn size(self) -> u64 {
        self.size
    }

    pub(crate) fn offset(self) -> u64 {
        (self.number - 1) * self.size
    }
}

#[derive(Debug)]
pub struct Paginated<T> {
    items: Vec<T>,
    page: Page,
    has_next: bool,
}

impl<T> Paginated<T> {
    /// A next page past `Page::MAX_OFFSET` would be refused, so none is offered.
    #[must_use]
    pub fn new(items: Vec<T>, page: Page, more: bool) -> Self {
        Self {
            items,
            page,
            has_next: more && Page::new(page.number + 1, page.size).is_ok(),
        }
    }

    #[must_use]
    pub fn items(&self) -> &[T] {
        &self.items
    }

    #[must_use]
    pub fn page(&self) -> Page {
        self.page
    }

    #[must_use]
    pub fn has_next(&self) -> bool {
        self.has_next
    }

    #[must_use]
    pub fn has_previous(&self) -> bool {
        self.page.number > 1
    }
}

impl<T: Serialize> Serialize for Paginated<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut page = serializer.serialize_struct("Paginated", 4)?;
        page.serialize_field(ITEMS, &self.items)?;
        page.serialize_field(PAGE, &self.page.number)?;
        page.serialize_field(SIZE, &self.page.size)?;
        page.serialize_field(HAS_NEXT, &self.has_next)?;
        page.end()
    }
}

impl<T: Show> Fields for Paginated<T> {
    fn name(&self, index: usize) -> Option<&str> {
        [ITEMS, PAGE, SIZE, HAS_NEXT].get(index).copied()
    }

    fn get(&self, name: &str) -> Option<Shown<'_>> {
        match name {
            ITEMS => Some(self.items.show()),
            PAGE => Some(self.page.number.show()),
            SIZE => Some(self.page.size.show()),
            HAS_NEXT => Some(Shown::Bool(self.has_next)),
            _ => None,
        }
    }
}

impl<T: Show> Show for Paginated<T> {
    fn show(&self) -> Shown<'_> {
        Shown::Record(self)
    }
}

impl<T: Show + Serialize + Send + Sync + 'static> Outcome for Paginated<T> {
    fn reply(self) -> Result<Reply> {
        Ok(Reply::Data(Box::new(Showing(self))))
    }
}

impl<T: Show + Serialize + Send + Sync + 'static> ReadOnly for Paginated<T> {}

impl<T: Source> Query<T> {
    /// Reads one record past the page instead of counting; ties are ordered by id.
    pub async fn paginate(self, read: &Read, page: Page) -> Result<Paginated<T>> {
        let mut criteria = self.criteria;
        if !criteria.sorts.iter().any(|sort| sort.column == "id") {
            criteria.sorts.push(crate::statement::SortKey::id(false));
        }
        criteria.limit = Some(page.size() + 1);
        criteria.offset = Some(page.offset());
        let mut items = criteria.all(read).await?;
        let size = usize::try_from(page.size()).expect("a page holds at most `Page::MAX_SIZE`");
        let has_next = items.len() > size;
        items.truncate(size);
        Ok(Paginated::new(items, page, has_next))
    }
}

#[cfg(test)]
mod tests {
    use sofie_core::Shown;

    use super::*;

    #[test]
    fn a_page_starts_after_the_ones_before_it() {
        assert_eq!(Page::new(3, 20).unwrap().offset(), 40);
        assert_eq!(Page::new(1, 1).unwrap().offset(), 0);
    }

    #[test]
    fn a_page_out_of_range_is_a_bad_request() {
        for (number, size) in [(0, 10), (1, 0), (1, Page::MAX_SIZE + 1), (u64::MAX, 100)] {
            assert!(
                matches!(Page::new(number, size), Err(Error::BadRequest(_))),
                "{number} {size}"
            );
        }
    }

    #[test]
    fn a_page_past_the_deepest_offset_asks_to_narrow_the_filter() {
        assert!(Page::new(101, 100).is_ok());

        let deep = Page::new(102, 100);

        assert!(
            matches!(&deep, Err(Error::BadRequest(message)) if message.contains("filter")),
            "{deep:?}"
        );
    }

    #[test]
    fn the_last_page_before_the_deepest_offset_links_to_no_next_one() {
        let last = Paginated::new(vec![1], Page::new(101, 100).unwrap(), true);

        assert!(!last.has_next());
    }

    #[test]
    fn a_page_serializes_its_items_and_position() {
        let page = Paginated::new(vec![1, 2], Page::new(2, 2).unwrap(), true);

        assert_eq!(
            serde_json::to_value(&page).unwrap(),
            serde_json::json!({ "items": [1, 2], "page": 2, "size": 2, "has_next": true })
        );
        assert!(page.has_previous());
    }

    #[test]
    fn a_page_of_shown_items_replies_them_typed_and_its_json_unchanged() {
        let at = sofie_core::Instant::parse("2026-10-11T22:30:00Z").unwrap();

        let reply = Paginated::new(vec![at], Page::new(1, 20).unwrap(), false)
            .reply()
            .unwrap();

        let items = match reply.shown() {
            Some(Shown::Record(page)) => page.get("items"),
            _ => None,
        };
        let first = match items {
            Some(Shown::List(items)) => items.item(0),
            _ => None,
        };
        assert!(matches!(first, Some(Shown::Instant(shown)) if shown == at));
        assert_eq!(
            reply.to_json().unwrap(),
            serde_json::json!({
                "items": ["2026-10-11T22:30:00Z"], "page": 1, "size": 20, "has_next": false
            })
        );
    }
}
