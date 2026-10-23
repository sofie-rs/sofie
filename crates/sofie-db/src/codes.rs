use sofie_core::{Cast, Error, Result, SqlType, SqlValue};
use sofie_values::{LATIN_COMBINING_ACCENTS, is_slug_character};
use unicode_normalization::UnicodeNormalization as _;

use crate::behavior::stored;
use crate::{
    Aggregate, Behavior, Column, Dialect, Field, Plan, Query, Read, Source, Step, Uses, Write,
};

const SLUG_LENGTH: usize = 100;
/// Crockford's base 32: no I, L, O or U, so a code read aloud is not misread.
const CODE_ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
const CODE_LENGTH: usize = 10;

pub struct Slugged {
    pub from: &'static str,
}

impl Behavior for Slugged {
    fn columns(&self) -> &'static [Field] {
        const { &[Field::new("slug", String::COLUMNS, false)] }
    }

    /// An unchanged `from` keeps its slug, so a later transliteration does not move the record.
    fn written(&self, write: &Write<'_>, values: &mut [SqlValue]) {
        let Some(SqlValue::Text(text)) = write.value(self.from) else {
            return;
        };
        let kept = matches!(values[0], SqlValue::Text(_))
            && write.loaded(self.from) == write.value(self.from);
        if !kept {
            values[0] = SqlValue::Text(slug(text));
        }
    }

    /// A slug taken now is no longer another record's previous one; a deleted record keeps none.
    fn planned(&self, write: &Write<'_>, plan: &mut Plan<'_>) -> Result<()> {
        let index = format!("{}_slug", write.table.name);
        plan.unique_column(write.table.schema, &index, self.from);
        let table = record_table(write);
        if write.deleted() == (false, true) {
            let params = vec![table, SqlValue::Uuid(write.id)];
            plan.push(previous_slugs(plan.dialect(), FORGET_RECORD, params));
            return Ok(());
        }
        let Some((previous, current)) = renamed(write) else {
            return Ok(());
        };
        if current.is_empty() {
            return Err(Error::invalid(
                self.from,
                "needs a letter or digit to name it by",
            ));
        }
        let params = vec![table.clone(), SqlValue::Text(current.to_owned())];
        plan.push(previous_slugs(plan.dialect(), FORGET_SLUG, params));
        if let Some(previous) = previous {
            let params = vec![
                table,
                SqlValue::Text(previous.to_owned()),
                SqlValue::Uuid(write.id),
                SqlValue::Timestamp(write.stamp.now.timestamp()),
            ];
            plan.push(previous_slugs(plan.dialect(), KEEP, params));
        }
        Ok(())
    }
}

const FORGET_RECORD: &str = "DELETE FROM {table} WHERE record_table = {1} AND record = {2}";
const FORGET_SLUG: &str = "DELETE FROM {table} WHERE record_table = {1} AND slug = {2}";
const KEEP: &str = "INSERT INTO {table} (record_table, slug, record, changed_at) \
    VALUES ({1}, {2}, {3}, {4}) ON CONFLICT (record_table, slug) \
    DO UPDATE SET record = excluded.record, changed_at = excluded.changed_at";

/// The slug the record leaves, if any, and the one it takes.
fn renamed<'w>(write: &'w Write<'_>) -> Option<(Option<&'w str>, &'w str)> {
    let Some(SqlValue::Text(current)) = write.value("slug") else {
        return None;
    };
    match write.loaded("slug") {
        Some(SqlValue::Text(previous)) if previous == current => None,
        Some(SqlValue::Text(previous)) if !previous.is_empty() => Some((Some(previous), current)),
        _ => Some((None, current)),
    }
}

fn record_table(write: &Write<'_>) -> SqlValue {
    SqlValue::Text(format!("{}.{}", write.table.schema, write.table.name))
}

fn previous_slugs(dialect: &dyn Dialect, template: &str, params: Vec<SqlValue>) -> Step {
    let mut sql = template.replace("{table}", &dialect.qualified("sofie", "previous_slug"));
    for index in 1..=params.len() {
        sql = sql.replace(&format!("{{{index}}}"), &dialect.placeholder(index));
    }
    Step::statement(sql, params, None)
}

fn slug(text: &str) -> String {
    let mut slug = String::new();
    for word in words(text).split('-').filter(|word| !word.is_empty()) {
        if slug.len() + word.len() + 1 > SLUG_LENGTH {
            if slug.is_empty() {
                slug.push_str(cut(word, SLUG_LENGTH));
            }
            break;
        }
        if !slug.is_empty() {
            slug.push('-');
        }
        slug.push_str(word);
    }
    slug
}

fn words(text: &str) -> String {
    let mut words = String::with_capacity(text.len());
    for character in text.nfc() {
        if let Some(latin) = latin(character) {
            words.push_str(latin);
            continue;
        }
        for lowered in character.to_lowercase() {
            match latin(lowered) {
                Some(latin) => words.push_str(latin),
                // A decomposed accent belongs to the Latin letter before it.
                None if LATIN_COMBINING_ACCENTS.contains(&lowered) => {}
                None if is_slug_character(lowered) => words.push(lowered),
                None => words.push('-'),
            }
        }
    }
    words
}

fn cut(word: &str, length: usize) -> &str {
    let mut end = length;
    while !word.is_char_boundary(end) {
        end -= 1;
    }
    &word[..end]
}

/// `İ` is matched before lowercasing, which would leave a dot above its `i`.
fn latin(character: char) -> Option<&'static str> {
    Some(match character {
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'ă' | 'ą' => "a",
        'æ' => "ae",
        'ç' | 'ć' | 'č' => "c",
        'ď' | 'đ' | 'ð' => "d",
        'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ę' | 'ě' => "e",
        'ğ' => "g",
        'ì' | 'í' | 'î' | 'ï' | 'ī' | 'ı' | 'İ' => "i",
        'ł' => "l",
        'ñ' | 'ń' | 'ň' => "n",
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' | 'ő' => "o",
        'œ' => "oe",
        'ř' => "r",
        'ß' => "ss",
        'ś' | 'ş' | 'š' | 'ș' => "s",
        'ț' | 'ţ' | 'ť' => "t",
        'þ' => "th",
        'ù' | 'ú' | 'û' | 'ü' | 'ū' | 'ů' | 'ű' => "u",
        'ý' | 'ÿ' => "y",
        'ź' | 'ż' | 'ž' => "z",
        _ => return cyrillic(character),
    })
}

fn cyrillic(character: char) -> Option<&'static str> {
    Some(match character {
        'а' => "a",
        'б' => "b",
        'в' => "v",
        'г' | 'ґ' => "g",
        'д' => "d",
        'е' | 'ё' | 'э' => "e",
        'є' => "ye",
        'ж' => "zh",
        'з' => "z",
        'и' | 'і' => "i",
        'ї' => "yi",
        'й' | 'ы' => "y",
        'к' => "k",
        'л' => "l",
        'м' => "m",
        'н' => "n",
        'о' => "o",
        'п' => "p",
        'р' => "r",
        'с' => "s",
        'т' => "t",
        'у' => "u",
        'ф' => "f",
        'х' => "kh",
        'ц' => "ts",
        'ч' => "ch",
        'ш' => "sh",
        'щ' => "shch",
        'ъ' | 'ь' => "",
        'ю' => "yu",
        'я' => "ya",
        _ => return None,
    })
}

pub trait Slugs: Aggregate {
    const SLUG: Column<Self, String> = Column::new("slug");

    fn slug(&self) -> &str {
        stored_text(self, "slug")
    }
}

impl<A: Uses<Slugged>> Slugs for A {}

impl<T: Source> Query<T>
where
    T::Base: Slugs,
{
    /// A slug that a record this query finds had before is `Error::Moved` to its current one.
    pub async fn by_slug(self, read: &Read, slug: &str) -> Result<T> {
        let named = self
            .clone()
            .where_(T::Base::SLUG.eq(slug.to_owned()))
            .first(read)
            .await?;
        if let Some(record) = named {
            return Ok(record);
        }
        let Some(current) = current_slug::<T::Base>(read, slug).await? else {
            return Err(Error::NotFound);
        };
        if self
            .where_(T::Base::SLUG.eq(current.clone()))
            .exists(read)
            .await?
        {
            return Err(Error::Moved {
                from: slug.to_owned(),
                to: current,
            });
        }
        Err(Error::NotFound)
    }
}

async fn current_slug<A: Aggregate>(read: &Read, previous: &str) -> Result<Option<String>> {
    let dialect = read.dialect();
    let sql = format!(
        "SELECT named.slug FROM {} AS moved JOIN {} AS named ON named.id = moved.record \
         WHERE moved.record_table = {} AND moved.slug = {}",
        dialect.qualified("sofie", "previous_slug"),
        dialect.qualified(A::MODULE, A::NAME),
        dialect.placeholder(1),
        dialect.placeholder(2)
    );
    let params = [
        SqlValue::Text(format!("{}.{}", A::MODULE, A::NAME)),
        SqlValue::Text(previous.to_owned()),
    ];
    let slugs = read
        .decoded(&sql, &params, 1, vec![Some(SqlType::Text)], String::decode)
        .await?;
    Ok(slugs.into_iter().next())
}

/// From the id's random bits; the migration adds the unique index.
pub struct UniqueCode;

impl Behavior for UniqueCode {
    fn columns(&self) -> &'static [Field] {
        const { &[Field::new("code", String::COLUMNS, false)] }
    }

    fn written(&self, write: &Write<'_>, values: &mut [SqlValue]) {
        if values[0] == SqlValue::Null {
            values[0] = SqlValue::Text(code(write.id.as_bytes()));
        }
    }
}

/// The id's last 50 bits, all random in a `UUIDv7`.
fn code(id: &[u8; 16]) -> String {
    let random = u64::from_be_bytes(id[8..].try_into().expect("eight bytes"));
    (0..CODE_LENGTH)
        .map(|index| {
            let digit = (random >> (5 * index)) & 0b1_1111;
            char::from(CODE_ALPHABET[usize::try_from(digit).expect("a five-bit digit")])
        })
        .collect()
}

pub trait Coded: Aggregate {
    const CODE: Column<Self, String> = Column::new("code");

    fn code(&self) -> &str {
        stored_text(self, "code")
    }
}

impl<A: Uses<UniqueCode>> Coded for A {}

fn stored_text<'a, A: Aggregate>(aggregate: &'a A, column: &str) -> &'a str {
    match stored(aggregate, column) {
        Some(SqlValue::Text(text)) => text,
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_slug_joins_lowercase_words_by_hyphens() {
        assert_eq!(slug("  Blue Wool -- Scarf (2026)!"), "blue-wool-scarf-2026");
        assert_eq!(slug("a/b_c.d,e:f"), "a-b-c-d-e-f");
        assert_eq!(slug("!?"), "");
    }

    #[test]
    fn a_slug_spells_turkish_german_and_accented_latin_letters_in_ascii() {
        assert_eq!(slug("Öğle Yemeği"), "ogle-yemegi");
        assert_eq!(
            slug("İstanbul'da Çay Şöleni ılık"),
            "istanbul-da-cay-soleni-ilik"
        );
        assert_eq!(slug("Große Straße für Äpfel"), "grosse-strasse-fur-apfel");
        assert_eq!(
            slug("Crème brûlée à la française"),
            "creme-brulee-a-la-francaise"
        );
        assert_eq!(slug("Cre\u{300}me"), "creme");
        assert_eq!(
            slug("Smørrebrød Æble Œuvre Łódź Đak Ñandú Åre"),
            "smorrebrod-aeble-oeuvre-lodz-dak-nandu-are"
        );
    }

    #[test]
    fn a_slug_spells_cyrillic_in_latin_and_keeps_other_scripts() {
        assert_eq!(slug("Привет, мир!"), "privet-mir");
        assert_eq!(slug("Щедрий вечір, Україно"), "shchedriy-vechir-ukrayino");
        assert_eq!(slug("关于我们 2026"), "关于我们-2026");
        assert_eq!(slug("Ελλάδα"), "ελλάδα");
    }

    #[test]
    fn a_decomposed_title_slugs_as_its_composed_form() {
        assert_eq!(slug("Мой край"), slug("Мои\u{306} край"));
        assert_eq!(slug("Ελλάδα"), slug("Ελλα\u{301}δα"));
    }

    #[test]
    fn a_slug_keeps_the_marks_of_other_scripts_in_their_word() {
        assert_eq!(slug("नमस्ते दुनिया"), "नमस्ते-दुनिया");
        assert_eq!(slug("ไม่ ใช่"), "ไม่-ใช่");
        assert_eq!(slug("关于 — 我们 🎉 «x»"), "关于-我们-x");
    }

    #[test]
    fn a_slug_stops_before_the_word_that_passes_its_length() {
        assert_eq!(slug(&"word ".repeat(30)).len(), 99);
        let long = slug(&"ğüş ".repeat(40));
        assert!(long.len() <= SLUG_LENGTH && long.ends_with("gus"), "{long}");
        let chinese = slug(&"关".repeat(40));
        assert_eq!(chinese, "关".repeat(33));
    }

    #[test]
    fn a_code_spells_the_random_bits_of_the_id() {
        let first = code(&[0; 16]);
        let other = code(&[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]);

        assert_eq!(first, "0000000000");
        assert_eq!(other, "1000000000");
    }
}
