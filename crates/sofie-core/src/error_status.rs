use crate::Error;

/// One table both ways: sofie-http answers an error with its status and a remote caller reads
/// the status back into the same kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorStatus {
    BadRequest,
    NotFound,
    Conflict,
    Unprocessable,
    TooManyRequests,
    Internal,
    Unavailable,
}

const CODES: [(ErrorStatus, u16); 7] = [
    (ErrorStatus::BadRequest, 400),
    (ErrorStatus::NotFound, 404),
    (ErrorStatus::Conflict, 409),
    (ErrorStatus::Unprocessable, 422),
    (ErrorStatus::TooManyRequests, 429),
    (ErrorStatus::Internal, 500),
    (ErrorStatus::Unavailable, 503),
];

impl ErrorStatus {
    #[must_use]
    pub fn of(error: &Error) -> Self {
        match error {
            Error::NotFound | Error::Moved { .. } => Self::NotFound,
            Error::Conflict => Self::Conflict,
            Error::BadRequest(_) => Self::BadRequest,
            Error::Invalid(_) | Error::Rule(_) => Self::Unprocessable,
            Error::TooManyRequests(_) => Self::TooManyRequests,
            Error::Unavailable(_) => Self::Unavailable,
            Error::Internal { .. } => Self::Internal,
        }
    }

    #[must_use]
    pub fn code(self) -> u16 {
        CODES
            .iter()
            .find(|(status, _)| *status == self)
            .map(|(_, code)| *code)
            .expect("every status is in the table")
    }

    #[must_use]
    pub fn from_code(code: u16) -> Option<Self> {
        CODES
            .iter()
            .find(|(_, candidate)| *candidate == code)
            .map(|(status, _)| *status)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn reads_back_the_status_each_error_is_answered_with() {
        let errors = [
            (Error::BadRequest(String::new()), 400),
            (Error::NotFound, 404),
            (Error::Conflict, 409),
            (Error::Rule(String::new()), 422),
            (Error::TooManyRequests(Duration::ZERO), 429),
            (Error::internal_message(""), 500),
            (Error::Unavailable(String::new()), 503),
        ];

        for (error, code) in errors {
            let status = ErrorStatus::of(&error);
            assert_eq!(status.code(), code);
            assert_eq!(ErrorStatus::from_code(code), Some(status));
        }
        assert_eq!(ErrorStatus::from_code(418), None);
    }
}
