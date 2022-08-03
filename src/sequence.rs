use std::ops::Deref;

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub struct Seq<T>(T);

impl Seq<u32> {
    pub fn inc(&mut self) -> u32 {
        self.0 = self.0.wrapping_add(1);
        self.0
    }
}

impl From<u32> for Seq<u32> {
    fn from(v: u32) -> Self {
        Self(v)
    }
}

impl<T: Default> Seq<T> {
    pub fn new() -> Self {
        Self(T::default())
    }
}

impl<T: Default> Default for Seq<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Deref for Seq<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

pub type Seq32 = Seq<u32>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_seq() {
        let mut s = Seq32::new();
        assert_eq!(s.0, 0);
        s.inc();
        assert_eq!(s.0, 1);

        let mut s: Seq32 = u32::MAX.into();
        assert_eq!(s.0, u32::MAX);
        s.inc();
        assert_eq!(s.0, 0);
    }
}
