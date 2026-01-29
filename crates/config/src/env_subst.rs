/// Replace `${ENV_VAR}` placeholders in config string values.
///
/// Unresolvable variables are left as-is.
pub fn substitute_env(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '$' && chars.peek() == Some(&'{') {
            chars.next(); // consume '{'
            let mut var_name = String::new();
            let mut closed = false;
            for c in chars.by_ref() {
                if c == '}' {
                    closed = true;
                    break;
                }
                var_name.push(c);
            }
            if closed && !var_name.is_empty() {
                match std::env::var(&var_name) {
                    Ok(val) => result.push_str(&val),
                    Err(_) => {
                        // Leave unresolved placeholder as-is.
                        result.push_str("${");
                        result.push_str(&var_name);
                        result.push('}');
                    }
                }
            } else {
                // Malformed — emit literal.
                result.push_str("${");
                result.push_str(&var_name);
            }
        } else {
            result.push(ch);
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn substitutes_known_var() {
        unsafe { std::env::set_var("MOLTIS_TEST_VAR", "hello") };
        assert_eq!(substitute_env("key=${MOLTIS_TEST_VAR}"), "key=hello");
        unsafe { std::env::remove_var("MOLTIS_TEST_VAR") };
    }

    #[test]
    fn leaves_unknown_var() {
        assert_eq!(
            substitute_env("${MOLTIS_NONEXISTENT_XYZ}"),
            "${MOLTIS_NONEXISTENT_XYZ}"
        );
    }

    #[test]
    fn no_placeholders() {
        assert_eq!(substitute_env("plain text"), "plain text");
    }
}
