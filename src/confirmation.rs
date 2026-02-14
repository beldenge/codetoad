#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmationOperation {
    File,
    Bash,
}

#[cfg(test)]
mod tests {
    use super::ConfirmationOperation;

    #[test]
    fn operations_are_distinct_and_debuggable() {
        assert_ne!(
            format!("{:?}", ConfirmationOperation::File),
            format!("{:?}", ConfirmationOperation::Bash)
        );
        assert_eq!(format!("{:?}", ConfirmationOperation::File), "File");
        assert_eq!(format!("{:?}", ConfirmationOperation::Bash), "Bash");
    }
}
