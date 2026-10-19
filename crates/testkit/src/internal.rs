//! Small helpers shared across modules that are not part of the public API.

use std::any::Any;

/// Extract the message from a caught panic payload.
///
/// Panics are how assertion helpers report failures, so helpers that catch
/// a closure's panic (and re-report it with context) need to read the
/// payload. `panic!` produces either a `&'static str` or a formatted
/// `String`; anything else gets a placeholder rather than being dropped.
pub(crate) fn panic_message(payload: &(dyn Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "<non-string panic payload>".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_both_string_payload_shapes_and_rejects_others() {
        let owned: Box<dyn Any + Send> = Box::new(String::from("owned"));
        let borrowed: Box<dyn Any + Send> = Box::new("borrowed");
        let other: Box<dyn Any + Send> = Box::new(7_u8);
        assert_eq!(panic_message(owned.as_ref()), "owned");
        assert_eq!(panic_message(borrowed.as_ref()), "borrowed");
        assert_eq!(panic_message(other.as_ref()), "<non-string panic payload>");
    }
}
