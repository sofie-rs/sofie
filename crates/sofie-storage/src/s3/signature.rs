//! AWS Signature Version 4, as S3 documents it:
//! <https://docs.aws.amazon.com/AmazonS3/latest/API/sig-v4-authenticating-requests.html>.

use ring::{digest, hmac};
use sofie_core::Timestamp;
use sofie_core::encoding::{hex, percent};

const ALGORITHM: &str = "AWS4-HMAC-SHA256";
const SERVICE: &str = "s3";
const TERMINATOR: &str = "aws4_request";
pub(crate) const UNSIGNED_PAYLOAD: &str = "UNSIGNED-PAYLOAD";

pub(crate) struct Credentials {
    pub(crate) access_key_id: String,
    pub(crate) secret_access_key: String,
    pub(crate) region: String,
}

/// `path` is encoded already; query names and values are encoded here.
pub(crate) struct Canonical<'a> {
    pub(crate) method: &'a str,
    pub(crate) path: &'a str,
    pub(crate) query: Vec<(String, String)>,
    /// Lowercase names.
    pub(crate) headers: Vec<(String, String)>,
    pub(crate) payload_hash: &'a str,
}

impl Canonical<'_> {
    fn signed_headers(&self) -> String {
        let names: Vec<&str> = self.headers.iter().map(|(name, _)| name.as_str()).collect();
        names.join(";")
    }

    fn request(&self) -> String {
        let mut headers = String::new();
        for (name, value) in &self.headers {
            headers.push_str(name);
            headers.push(':');
            headers.push_str(value.trim());
            headers.push('\n');
        }
        format!(
            "{}\n{}\n{}\n{headers}\n{}\n{}",
            self.method,
            self.path,
            query_string(&self.query),
            self.signed_headers(),
            self.payload_hash
        )
    }

    fn sorted(mut self) -> Self {
        self.headers.sort();
        self
    }
}

impl Credentials {
    pub(crate) fn authorization(&self, canonical: Canonical<'_>, at: Timestamp) -> String {
        let canonical = canonical.sorted();
        let signature = self.signature(&canonical, at);
        format!(
            "{ALGORITHM} Credential={}/{}, SignedHeaders={}, Signature={signature}",
            self.access_key_id,
            self.scope(at),
            canonical.signed_headers()
        )
    }

    /// The query of a link that performs `method` on `path` for anyone holding it until it expires.
    pub(crate) fn presigned_query(
        &self,
        method: &str,
        path: &str,
        host: &str,
        mut query: Vec<(String, String)>,
        at: Timestamp,
        expires_in_seconds: u64,
    ) -> String {
        query.extend([
            ("X-Amz-Algorithm".to_owned(), ALGORITHM.to_owned()),
            (
                "X-Amz-Credential".to_owned(),
                format!("{}/{}", self.access_key_id, self.scope(at)),
            ),
            ("X-Amz-Date".to_owned(), amz_date(at)),
            ("X-Amz-Expires".to_owned(), expires_in_seconds.to_string()),
            ("X-Amz-SignedHeaders".to_owned(), "host".to_owned()),
        ]);
        let canonical = Canonical {
            method,
            path,
            query,
            headers: vec![("host".to_owned(), host.to_owned())],
            payload_hash: UNSIGNED_PAYLOAD,
        };
        let signature = self.signature(&canonical, at);
        format!(
            "{}&X-Amz-Signature={signature}",
            query_string(&canonical.query)
        )
    }

    fn signature(&self, canonical: &Canonical<'_>, at: Timestamp) -> String {
        let to_sign = format!(
            "{ALGORITHM}\n{}\n{}\n{}",
            amz_date(at),
            self.scope(at),
            sha256_hex(canonical.request().as_bytes())
        );
        let key = signing_key(&self.secret_access_key, &date(at), &self.region, SERVICE);
        let key = hmac::Key::new(hmac::HMAC_SHA256, key.as_ref());
        hex::encode(hmac::sign(&key, to_sign.as_bytes()).as_ref())
    }

    fn scope(&self, at: Timestamp) -> String {
        format!("{}/{}/{SERVICE}/{TERMINATOR}", date(at), self.region)
    }
}

fn signing_key(secret: &str, date: &str, region: &str, service: &str) -> hmac::Tag {
    let first = hmac::Key::new(hmac::HMAC_SHA256, format!("AWS4{secret}").as_bytes());
    let mut tag = hmac::sign(&first, date.as_bytes());
    for part in [region, service, TERMINATOR] {
        tag = hmac::sign(
            &hmac::Key::new(hmac::HMAC_SHA256, tag.as_ref()),
            part.as_bytes(),
        );
    }
    tag
}

pub(crate) fn amz_date(at: Timestamp) -> String {
    at.strftime("%Y%m%dT%H%M%SZ").to_string()
}

fn date(at: Timestamp) -> String {
    at.strftime("%Y%m%d").to_string()
}

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(digest::digest(&digest::SHA256, bytes).as_ref())
}

pub(crate) fn query_string(query: &[(String, String)]) -> String {
    let mut encoded: Vec<(String, String)> = query
        .iter()
        .map(|(name, value)| (uri_encode(name, false), uri_encode(value, false)))
        .collect();
    encoded.sort();
    let pairs: Vec<String> = encoded
        .into_iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect();
    pairs.join("&")
}

pub(crate) fn uri_encode(text: &str, keeps_slashes: bool) -> String {
    if keeps_slashes {
        percent::encode(text, |byte| percent::is_unreserved(byte) || byte == b'/')
    } else {
        percent::encode(text, percent::is_unreserved)
    }
}

/// The inputs and signatures of the examples in AWS's documentation.
#[cfg(test)]
mod tests {
    use super::*;

    const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

    fn example() -> Credentials {
        Credentials {
            access_key_id: "AKIAIOSFODNN7EXAMPLE".to_owned(),
            secret_access_key: "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY".to_owned(),
            region: "us-east-1".to_owned(),
        }
    }

    fn may_24_2013() -> Timestamp {
        "2013-05-24T00:00:00Z".parse().unwrap()
    }

    fn headers(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect()
    }

    fn signature_of(authorization: &str) -> &str {
        authorization.rsplit("Signature=").next().unwrap()
    }

    #[test]
    fn derives_the_documented_signing_key() {
        let key = signing_key(
            "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY",
            "20120215",
            "us-east-1",
            "iam",
        );

        assert_eq!(
            hex::encode(key.as_ref()),
            "f4780e2d9f65fa895f9c67b32ce1baf0b0d8a43505a000a1a9e090d414db404d"
        );
    }

    #[test]
    fn signs_the_documented_get_object_request() {
        let canonical = Canonical {
            method: "GET",
            path: "/test.txt",
            query: Vec::new(),
            headers: headers(&[
                ("host", "examplebucket.s3.amazonaws.com"),
                ("range", "bytes=0-9"),
                ("x-amz-content-sha256", EMPTY_SHA256),
                ("x-amz-date", "20130524T000000Z"),
            ]),
            payload_hash: EMPTY_SHA256,
        };
        let authorization = example().authorization(canonical, may_24_2013());

        assert!(authorization.starts_with(
            "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20130524/us-east-1/s3/aws4_request, \
             SignedHeaders=host;range;x-amz-content-sha256;x-amz-date, "
        ));
        assert_eq!(
            signature_of(&authorization),
            "f0e8bdb87c964420e857bd35b5d6ed310bd44f0170aba48dd91039c6036bdb41"
        );
    }

    #[test]
    fn signs_the_documented_put_object_request() {
        let payload = sha256_hex(b"Welcome to Amazon S3.");
        let canonical = Canonical {
            method: "PUT",
            path: &uri_encode("/test$file.text", true),
            query: Vec::new(),
            headers: headers(&[
                ("date", "Fri, 24 May 2013 00:00:00 GMT"),
                ("host", "examplebucket.s3.amazonaws.com"),
                ("x-amz-content-sha256", &payload),
                ("x-amz-date", "20130524T000000Z"),
                ("x-amz-storage-class", "REDUCED_REDUNDANCY"),
            ]),
            payload_hash: &payload,
        };

        assert_eq!(
            signature_of(&example().authorization(canonical, may_24_2013())),
            "98ad721746da40c64f1a55b78f14c238d841ea1380cd77a1b5971af0ece108bd"
        );
    }

    #[test]
    fn signs_the_documented_list_objects_request() {
        let canonical = Canonical {
            method: "GET",
            path: "/",
            query: vec![
                ("max-keys".to_owned(), "2".to_owned()),
                ("prefix".to_owned(), "J".to_owned()),
            ],
            headers: headers(&[
                ("host", "examplebucket.s3.amazonaws.com"),
                ("x-amz-content-sha256", EMPTY_SHA256),
                ("x-amz-date", "20130524T000000Z"),
            ]),
            payload_hash: EMPTY_SHA256,
        };

        assert_eq!(
            signature_of(&example().authorization(canonical, may_24_2013())),
            "34b48302e7b5fa45bde8084f4b7868a86f0a534bc59db6670ed5711ef69dc6f7"
        );
    }

    #[test]
    fn presigns_the_documented_link() {
        let query = example().presigned_query(
            "GET",
            "/test.txt",
            "examplebucket.s3.amazonaws.com",
            Vec::new(),
            may_24_2013(),
            86_400,
        );

        assert_eq!(
            query,
            "X-Amz-Algorithm=AWS4-HMAC-SHA256\
             &X-Amz-Credential=AKIAIOSFODNN7EXAMPLE%2F20130524%2Fus-east-1%2Fs3%2Faws4_request\
             &X-Amz-Date=20130524T000000Z&X-Amz-Expires=86400&X-Amz-SignedHeaders=host\
             &X-Amz-Signature=aeeed9bbccd4d02ee5c0109b86d86835f995330da4c265957d157751f604d404"
        );
    }
}
