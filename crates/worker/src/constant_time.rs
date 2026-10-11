//! Comparing secrets without a timing signal. The owner's password, the session cookie's tag
//! and a recorder's stream secret all go through [`eq`].

/// Whether `a` equals `b`, in time independent of where they differ (the length still shows).
pub fn eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0, |diff, (x, y)| diff | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::eq;

    #[test]
    fn only_equal_bytes_match() {
        assert!(eq(b"", b""));
        assert!(eq(b"secret", b"secret"));
        assert!(!eq(b"secret", b"secreT"));
        assert!(!eq(b"secret", b"Secret"));
        assert!(!eq(b"secret", b"secre"));
        assert!(!eq(b"", b"x"));
    }
}
