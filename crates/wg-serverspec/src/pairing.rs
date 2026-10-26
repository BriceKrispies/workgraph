//! Pairing a LAN device (X4, X5): the code `serve` prints, its check and lockout, and the
//! `/pair` page and answers.
//!
//! The pairing state is a value: [`Pairing::attempt`] returns the next state with the outcome,
//! and the shell keeps the current one behind its lock.

use subtle::ConstantTimeEq;

use crate::form::form_first;
use crate::reply::Reply;
use crate::security::{Incoming, Policy, device_set_cookie, trusted_browser};
use crate::text::html_escape;

/// The characters of a pairing code: no `I`, `L`, `O`, `0` or `1`.
pub const PAIRING_ALPHABET: &str = "ABCDEFGHJKMNPQRSTUVWXYZ23456789";
/// Wrong codes after which pairing stays off until `serve` restarts.
pub const PAIRING_MAX_FAILURES: u32 = 10;
/// The message of a wrong code.
pub const WRONG_CODE: &str = "That code is not right.";
/// The message once pairing is locked.
pub const LOCKED: &str = "Too many wrong codes. Restart workgraph serve to pair again.";

/// `_new_pairing_code()`: eight characters of the alphabet as `XXXX-XXXX`, drawn from `entropy`
/// by rejection sampling (a byte is used only when it is below the largest multiple of the
/// alphabet size). `None` when the entropy ran out first; the shell then draws more.
pub fn pairing_code(entropy: &[u8]) -> Option<String> {
    let alphabet: Vec<char> = PAIRING_ALPHABET.chars().collect();
    let size = alphabet.len();
    let limit = 256 - 256 % size;
    let chars: Vec<char> = entropy
        .iter()
        .map(|byte| usize::from(*byte))
        .filter(|byte| *byte < limit)
        .filter_map(|byte| alphabet.get(byte % size).copied())
        .take(8)
        .collect();
    (chars.len() == 8).then(|| {
        let (head, tail) = chars.split_at(4);
        format!(
            "{}-{}",
            head.iter().collect::<String>(),
            tail.iter().collect::<String>()
        )
    })
}

/// `_normalise_code`: upper case, letters and digits only.
pub fn normalise_code(code: &str) -> String {
    code.to_uppercase()
        .chars()
        .filter(|ch| ch.is_alphanumeric())
        .collect()
}

/// What a pairing attempt found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Attempt {
    Paired,
    Wrong,
    Locked,
}

/// `LanAccess`'s pairing state: the code and the wrong guesses so far.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pairing {
    pub code: String,
    pub failures: u32,
}

impl Pairing {
    /// A fresh state for `code`.
    pub fn new(code: &str) -> Pairing {
        Pairing {
            code: code.to_owned(),
            failures: 0,
        }
    }

    /// `LanAccess.locked`.
    pub fn locked(&self) -> bool {
        self.failures >= PAIRING_MAX_FAILURES
    }

    /// `LanAccess.try_pair(code)`: a constant-time comparison of the normalised codes; a wrong
    /// one counts towards the lock.
    pub fn attempt(&self, code: &str) -> (Pairing, Attempt) {
        if self.locked() {
            return (self.clone(), Attempt::Locked);
        }
        let given = normalise_code(code);
        let expected = normalise_code(&self.code);
        if bool::from(given.as_bytes().ct_eq(expected.as_bytes())) {
            (self.clone(), Attempt::Paired)
        } else {
            (
                Pairing {
                    failures: self.failures.saturating_add(1),
                    ..self.clone()
                },
                Attempt::Wrong,
            )
        }
    }
}

/// `_pair_html(error)`: the pairing form, with the error message when there is one.
pub fn pair_html(error: Option<&str>) -> String {
    let message = error
        .map(|text| format!("<p class=\"error\">{}</p>", html_escape(text)))
        .unwrap_or_default();
    format!(
        "<!doctype html><html lang='en'><head><meta charset='utf-8'>\
<meta name='viewport' content='width=device-width, initial-scale=1'>\
<meta name='color-scheme' content='light dark'><title>Pair device - Workgraph</title>\
<style>body{{font-family:system-ui,sans-serif;margin:0;padding:24px 16px;display:flex;\
justify-content:center}}main{{width:100%;max-width:360px}}h1{{font-size:1.4rem}}\
input,button{{font-size:1.2rem;padding:12px;width:100%;box-sizing:border-box;margin-top:12px;\
border-radius:8px;border:1px solid #888}}input{{letter-spacing:.15em;text-transform:uppercase}}\
button{{cursor:pointer;background:#2563eb;color:#fff;border-color:#2563eb}}\
.error{{color:#dc2626}}</style></head><body><main><h1>Pair this device</h1>\
<p>Enter the pairing code that <code>workgraph serve</code> printed on the computer \
running Workgraph.</p>\
{message}<form method='post' action='/pair'>\
<input name='code' autocomplete='one-time-code' autocapitalize='characters' \
placeholder='XXXX-XXXX' required autofocus><button type='submit'>Pair</button>\
</form></main></body></html>"
    )
}

/// `GET /pair`: a trusted browser goes home; anyone else gets the form.
pub fn pair_form(policy: &Policy, req: &Incoming) -> Reply {
    if trusted_browser(policy, req) {
        return Reply::see_other("/");
    }
    Reply::html(200, &pair_html(None)).with_header("cache-control", "no-store")
}

/// The code a `POST /pair` body submits.
pub fn submitted_code(body: &[u8]) -> String {
    form_first(&String::from_utf8_lossy(body), "code")
}

/// `POST /pair`, given the pairing state (`None` without LAN serving): the reply and the next
/// state.
pub fn pair_submit(
    policy: &Policy,
    state: Option<&Pairing>,
    body: &[u8],
) -> (Reply, Option<Pairing>) {
    let Some(state) = state.filter(|_| policy.lan.is_some()) else {
        return (Reply::see_other("/"), None);
    };
    if state.locked() {
        return (
            Reply::html(429, &pair_html(Some(LOCKED))),
            Some(state.clone()),
        );
    }
    let (next, outcome) = state.attempt(&submitted_code(body));
    let reply = match outcome {
        Attempt::Paired => {
            Reply::see_other("/").with_header("set-cookie", &device_set_cookie(&policy.token))
        }
        Attempt::Wrong => Reply::html(403, &pair_html(Some(WRONG_CODE))),
        Attempt::Locked => Reply::html(429, &pair_html(Some(LOCKED))),
    };
    (reply, Some(next))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_from_entropy() {
        let code = pairing_code(&[0, 1, 2, 3, 30, 31, 255, 247, 4, 5]).unwrap();
        // Bytes from 248 up are rejected; 31 wraps to the first character, 247 to the last.
        assert_eq!(code, "ABCD-9A9E");
        assert!(pairing_code(&[255; 64]).is_none());
        assert_eq!(normalise_code("abcd-efgh"), "ABCDEFGH");
    }

    #[test]
    fn locks_after_ten_wrong_codes() {
        let state = (0..PAIRING_MAX_FAILURES).fold(Pairing::new("ABCD-EFGH"), |state, _| {
            let (next, outcome) = state.attempt("WRONG");
            assert_eq!(outcome, Attempt::Wrong);
            next
        });
        assert!(state.locked());
        assert_eq!(state.attempt("ABCD-EFGH").1, Attempt::Locked);
        assert_eq!(
            Pairing::new("ABCD-EFGH").attempt("abcd efgh").1,
            Attempt::Paired
        );
    }
}
