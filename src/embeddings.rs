//! The text embedded for a symbol in the embeddings stage (5.1).
//!
//! The text is the symbol's fully qualified name and its description, one per
//! line, so a query can match a class or method name as well as what it does.
//! [`EmbeddingConfig::parent_description`] adds the enclosing type's
//! description to a method's or constructor's text. The text is the
//! embedding cache key (with the model), so changing its format re-embeds
//! every symbol once; it never touches the chat cache.

use crate::config::EmbeddingConfig;

/// One symbol the embeddings stage embeds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddingInput<'a> {
    pub fqn: &'a str,
    /// The symbol's own description.
    pub description: &'a str,
    /// The enclosing type's description, for a method or constructor that
    /// has one; `None` for a type and for a member whose type has none.
    pub parent_description: Option<&'a str>,
}

/// The text embedded for `input`: `fqn`, a newline, the description, and
/// with `config.parent_description` a newline and the enclosing type's
/// description. Each part is trimmed; a blank parent description is left out.
pub fn embedding_text(input: &EmbeddingInput<'_>, config: &EmbeddingConfig) -> String {
    let mut text = format!("{}\n{}", input.fqn.trim(), input.description.trim());
    if config.parent_description
        && let Some(parent) = input.parent_description.map(str::trim)
        && !parent.is_empty()
    {
        text.push('\n');
        text.push_str(parent);
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    const MEMBER: EmbeddingInput<'static> = EmbeddingInput {
        fqn: "com.acme.UserService#find(String)",
        description: " Loads a user by email. ",
        parent_description: Some("Manages users."),
    };

    #[test]
    fn default_text_is_fqn_and_description() {
        assert_eq!(
            embedding_text(&MEMBER, &EmbeddingConfig::default()),
            "com.acme.UserService#find(String)\nLoads a user by email."
        );
    }

    #[test]
    fn parent_description_is_appended_only_when_configured() {
        let config = EmbeddingConfig {
            parent_description: true,
        };
        assert_eq!(
            embedding_text(&MEMBER, &config),
            "com.acme.UserService#find(String)\nLoads a user by email.\nManages users."
        );
        for parent in [None, Some("  ")] {
            let input = EmbeddingInput {
                parent_description: parent,
                ..MEMBER
            };
            assert_eq!(
                embedding_text(&input, &config),
                "com.acme.UserService#find(String)\nLoads a user by email."
            );
        }
    }
}
