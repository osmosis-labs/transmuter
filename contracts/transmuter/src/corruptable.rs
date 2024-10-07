pub trait Corruptable {
    fn is_corrupted(&self) -> bool;
    fn mark_as_corrupted(&mut self) -> &mut Self;
    fn unmark_as_corrupted(&mut self) -> &mut Self;
}
