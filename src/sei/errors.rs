use std::fmt;

use unicode_normalization::UnicodeNormalization;
use unicode_properties::{GeneralCategory, UnicodeGeneralCategory};

use super::wire::ROUTE_LOGIN;
use crate::gotext::{go_quote, to_lower, trim};
use crate::password::PasswordCommandError;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    InvalidCredentials,
    Unauthorized,
    ModuleNotFound,
    ModuleUnavailable,
    Server,
    Redirect,
    UnexpectedResponse,
    OrgNotFound,
    Refused,
    UnknownUnit,
    PasswordCommand,
}

#[derive(Clone, Debug)]
pub struct ApiError {
    pub route: String,
    pub status: u16,
    pub message: String,
    pub kind: ErrorKind,
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.message.is_empty() {
            return write!(f, "sei: {} answered HTTP {}", self.route, self.status);
        }
        write!(f, "sei: {}: {}", self.route, self.message)
    }
}

impl std::error::Error for ApiError {}

#[derive(Debug, thiserror::Error)]
pub enum SeiError {
    #[error(transparent)]
    Api(ApiError),
    #[error("sei: {context}: {detail}")]
    Transport {
        context: String,
        detail: String,
        timed_out: bool,
    },
    #[error("sei: read the {route} answer: {detail}")]
    Decode {
        route: String,
        detail: String,
        timed_out: bool,
    },
    #[error("sei: /autenticar: {0}")]
    Password(#[source] PasswordCommandError),
    #[error("sei: /autenticar: SEI did not return a session token")]
    NoToken,
    #[error("sei: the process list went past 400 pages")]
    TooManyPages,
    #[error(
        "the mod-wssei module was not found on this SEI: tried {first} and {second}; set SEI_API_URL if the module lives elsewhere"
    )]
    ModuleNotFound { first: String, second: String },
    #[error(
        "agency not found on the SEI login page: {} (the orgs subcommand lists the available acronyms)",
        go_quote(.0)
    )]
    OrgNotFound(String),
    #[error("sei: the login page has no agency list (selOrgao field)")]
    NoAgencyList,
    #[error("sei: the login page answered HTTP {0}")]
    LoginPageStatus(u16),
    #[error(
        "unit not found among the user's units: {} (yours are {})",
        go_quote(.acronym),
        .yours.join(", ")
    )]
    UnknownUnit { acronym: String, yours: Vec<String> },
}

impl SeiError {
    pub fn kind(&self) -> Option<ErrorKind> {
        match self {
            SeiError::Api(error) => Some(error.kind),
            SeiError::Password(_) => Some(ErrorKind::PasswordCommand),
            SeiError::ModuleNotFound { .. } => Some(ErrorKind::ModuleNotFound),
            SeiError::OrgNotFound(_) => Some(ErrorKind::OrgNotFound),
            SeiError::UnknownUnit { .. } => Some(ErrorKind::UnknownUnit),
            _ => None,
        }
    }

    pub(crate) fn timed_out(&self) -> bool {
        matches!(
            self,
            SeiError::Transport {
                timed_out: true,
                ..
            } | SeiError::Decode {
                timed_out: true,
                ..
            }
        )
    }
}

pub(crate) fn classify(route: &str, status: u16, message: &str) -> ErrorKind {
    let folded = fold(message);
    let rules = [
        (
            folded.starts_with("modulo inativo") || folded.starts_with("modulo incompativel"),
            ErrorKind::ModuleUnavailable,
        ),
        (status >= 500, ErrorKind::Server),
        (route == ROUTE_LOGIN, ErrorKind::InvalidCredentials),
        (
            status == 401 || status == 403 || folded.contains("token"),
            ErrorKind::Unauthorized,
        ),
    ];
    rules
        .into_iter()
        .find_map(|(matches, kind)| matches.then_some(kind))
        .unwrap_or(ErrorKind::Refused)
}

pub(crate) fn fold(text: &str) -> String {
    let stripped: String = text
        .chars()
        .stream_safe()
        .nfd()
        .filter(|c| c.general_category() != GeneralCategory::NonspacingMark)
        .stream_safe()
        .nfc()
        .collect();
    to_lower(trim(&stripped))
}

#[cfg(test)]
pub(super) mod tests {
    use std::error::Error;

    use serde::Deserialize;

    use super::*;
    use crate::golden;
    use crate::password::CommandFailure;

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct FoldCase {
        input: String,
        output: String,
    }

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct ClassifyCase {
        route: String,
        status: u16,
        message: String,
        kind: String,
    }

    pub(crate) fn kind_named(name: &str) -> ErrorKind {
        let kinds = [
            ("module_unavailable", ErrorKind::ModuleUnavailable),
            ("server", ErrorKind::Server),
            ("invalid_credentials", ErrorKind::InvalidCredentials),
            ("unauthorized", ErrorKind::Unauthorized),
            ("refused", ErrorKind::Refused),
            ("redirect", ErrorKind::Redirect),
            ("unexpected_response", ErrorKind::UnexpectedResponse),
        ];
        kinds
            .iter()
            .find(|(known, _)| *known == name)
            .map(|(_, kind)| *kind)
            .unwrap_or_else(|| panic!("unknown error kind {name:?} in a vector"))
    }

    #[test]
    fn fold_matches_go() {
        for case in golden::cases::<FoldCase>("fold", "") {
            assert_eq!(fold(&case.input), case.output, "fold {:?}", case.input);
        }
    }

    #[test]
    fn classify_matches_go() {
        for case in golden::cases::<ClassifyCase>("classify", "") {
            assert_eq!(
                classify(&case.route, case.status, &case.message),
                kind_named(&case.kind),
                "{} {} {:?}",
                case.route,
                case.status,
                case.message
            );
        }
    }

    #[test]
    fn fold_strips_only_nonspacing_marks() {
        let enclosing = '\u{20DD}';
        let spacing = '\u{0903}';
        assert_eq!(enclosing.general_category(), GeneralCategory::EnclosingMark);
        assert_eq!(spacing.general_category(), GeneralCategory::SpacingMark);
        assert_eq!(
            fold(&format!("A\u{0301}{enclosing}{spacing}")),
            format!("a{enclosing}{spacing}")
        );
    }

    fn password_error() -> PasswordCommandError {
        PasswordCommandError {
            program: "pass".to_owned(),
            exit_code: 1,
            stderr_bytes: 0,
            failure: CommandFailure::Exit,
        }
    }

    #[test]
    fn errors_read_like_go() {
        let api = |message: &str| ApiError {
            route: "/processo/listar".to_owned(),
            status: 403,
            message: message.to_owned(),
            kind: ErrorKind::Unauthorized,
        };
        let cases = [
            (
                SeiError::Api(api("Acesso negado!")),
                "sei: /processo/listar: Acesso negado!",
            ),
            (
                SeiError::Api(api("")),
                "sei: /processo/listar answered HTTP 403",
            ),
            (
                SeiError::Transport {
                    context: "decode the login page".to_owned(),
                    detail: "EOF".to_owned(),
                    timed_out: false,
                },
                "sei: decode the login page: EOF",
            ),
            (
                SeiError::Decode {
                    route: "/versao".to_owned(),
                    detail: "expected an object".to_owned(),
                    timed_out: false,
                },
                "sei: read the /versao answer: expected an object",
            ),
            (
                SeiError::Password(password_error()),
                "sei: /autenticar: SEI_PASSWORD_CMD: pass exited with code 1",
            ),
            (
                SeiError::NoToken,
                "sei: /autenticar: SEI did not return a session token",
            ),
            (
                SeiError::TooManyPages,
                "sei: the process list went past 400 pages",
            ),
            (
                SeiError::ModuleNotFound {
                    first: "https://h/sei/modulos/wssei/controlador_ws.php/api/v2".to_owned(),
                    second: "https://h/sei/modulos/mod-wssei/controlador_ws.php/api/v2".to_owned(),
                },
                "the mod-wssei module was not found on this SEI: tried \
                 https://h/sei/modulos/wssei/controlador_ws.php/api/v2 and \
                 https://h/sei/modulos/mod-wssei/controlador_ws.php/api/v2; \
                 set SEI_API_URL if the module lives elsewhere",
            ),
            (
                SeiError::OrgNotFound("se\"demo\u{a0}".to_owned()),
                "agency not found on the SEI login page: \"se\\\"demo\\u00a0\" \
                 (the orgs subcommand lists the available acronyms)",
            ),
            (
                SeiError::NoAgencyList,
                "sei: the login page has no agency list (selOrgao field)",
            ),
            (
                SeiError::LoginPageStatus(503),
                "sei: the login page answered HTTP 503",
            ),
            (
                SeiError::UnknownUnit {
                    acronym: "SEDEMO/X".to_owned(),
                    yours: vec!["SEDEMO/NUPROC".to_owned(), "SEDEMO/GAB".to_owned()],
                },
                "unit not found among the user's units: \"SEDEMO/X\" \
                 (yours are SEDEMO/NUPROC, SEDEMO/GAB)",
            ),
        ];
        for (error, text) in cases {
            assert_eq!(error.to_string(), text);
        }
    }

    #[test]
    fn kinds_follow_the_go_sentinels() {
        let api = ApiError {
            route: "/autenticar".to_owned(),
            status: 200,
            message: "Usuário ou Senha Inválida.".to_owned(),
            kind: ErrorKind::InvalidCredentials,
        };
        let cases = [
            (SeiError::Api(api), Some(ErrorKind::InvalidCredentials)),
            (
                SeiError::Password(password_error()),
                Some(ErrorKind::PasswordCommand),
            ),
            (
                SeiError::ModuleNotFound {
                    first: "a".to_owned(),
                    second: "b".to_owned(),
                },
                Some(ErrorKind::ModuleNotFound),
            ),
            (
                SeiError::OrgNotFound("x".to_owned()),
                Some(ErrorKind::OrgNotFound),
            ),
            (
                SeiError::UnknownUnit {
                    acronym: "x".to_owned(),
                    yours: Vec::new(),
                },
                Some(ErrorKind::UnknownUnit),
            ),
            (
                SeiError::Transport {
                    context: "c".to_owned(),
                    detail: "d".to_owned(),
                    timed_out: false,
                },
                None,
            ),
            (
                SeiError::Decode {
                    route: "r".to_owned(),
                    detail: "d".to_owned(),
                    timed_out: false,
                },
                None,
            ),
            (SeiError::NoToken, None),
            (SeiError::TooManyPages, None),
            (SeiError::NoAgencyList, None),
            (SeiError::LoginPageStatus(500), None),
        ];
        for (error, kind) in cases {
            assert_eq!(error.kind(), kind, "{error}");
        }
    }

    #[test]
    fn causes_are_kept_as_sources() {
        let password = SeiError::Password(password_error());
        assert_eq!(
            password.source().map(ToString::to_string),
            Some("SEI_PASSWORD_CMD: pass exited with code 1".to_owned())
        );
        let api = SeiError::Api(ApiError {
            route: "/versao".to_owned(),
            status: 500,
            message: String::new(),
            kind: ErrorKind::Server,
        });
        assert!(api.source().is_none());
    }
}
