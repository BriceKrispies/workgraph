//! Query strings and form bodies, as Python's `urllib.parse` splits them.

use crate::http::text::unquote;

/// One name or value: `+` is a space, then `%XX` escapes are decoded.
fn decode(part: &str) -> String {
    unquote(&part.replace('+', " "))
}

/// `parse_qsl(text, keep_blank_values=True)`, as Starlette's `QueryParams` reads the query
/// string: `&`-separated pairs in order; a pair without `=` has an empty value.
pub fn parse_qsl(text: &str) -> Vec<(String, String)> {
    text.split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| match pair.split_once('=') {
            Some((name, value)) => (decode(name), decode(value)),
            None => (decode(pair), String::new()),
        })
        .collect()
}

/// `QueryParams.get(name)`: the last value given for `name`.
pub fn query_get(pairs: &[(String, String)], name: &str) -> Option<String> {
    pairs
        .iter()
        .rev()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.clone())
}

/// `QueryParams.getlist(name)`: every value given for `name`, in order.
pub fn query_getlist(pairs: &[(String, String)], name: &str) -> Vec<String> {
    pairs
        .iter()
        .filter(|(key, _)| key == name)
        .map(|(_, value)| value.clone())
        .collect()
}

/// `parse_qs(text).get(name, [""])[0]`: the first non-blank value of `name` in a form body, or
/// the empty string (`parse_qs` drops blank values).
pub fn form_first(text: &str, name: &str) -> String {
    parse_qsl(text)
        .into_iter()
        .find(|(key, value)| key == name && !value.is_empty())
        .map(|(_, value)| value)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_like_parse_qsl() {
        assert_eq!(
            parse_qsl("a=1&&b&c=x+y%21&a=2"),
            vec![
                ("a".into(), "1".into()),
                ("b".into(), "".into()),
                ("c".into(), "x y!".into()),
                ("a".into(), "2".into()),
            ]
        );
        let pairs = parse_qsl("limit=abc&limit=7");
        assert_eq!(query_get(&pairs, "limit").as_deref(), Some("7"));
        assert_eq!(query_getlist(&pairs, "limit"), vec!["abc", "7"]);
        assert_eq!(
            form_first("code=&code=ABCD-EFGH&code=x", "code"),
            "ABCD-EFGH"
        );
        assert_eq!(form_first("other=1", "code"), "");
    }
}
