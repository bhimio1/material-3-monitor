//! Rolling histories for the charts: one fixed-length series per key.

use std::collections::{HashMap, VecDeque};

#[derive(Default)]
pub struct History {
    len: usize,
    series: HashMap<String, VecDeque<f32>>,
}

impl History {
    pub fn new(len: usize) -> Self {
        Self { len: len.max(2), series: HashMap::new() }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    /// Change how many samples are kept, trimming the oldest.
    pub fn set_len(&mut self, len: usize) {
        self.len = len.max(2);
        for s in self.series.values_mut() {
            while s.len() > self.len {
                s.pop_front();
            }
        }
    }

    pub fn push(&mut self, key: &str, value: f32) {
        let len = self.len;
        let s = self.series.entry(key.to_string()).or_insert_with(|| VecDeque::with_capacity(len));
        if s.len() == len {
            s.pop_front();
        }
        s.push_back(if value.is_finite() { value } else { 0.0 });
    }

    pub fn get(&self, key: &str) -> Option<&VecDeque<f32>> {
        self.series.get(key)
    }

    /// Values of `key`, or an empty slice-like iterator.
    pub fn values(&self, key: &str) -> Vec<f32> {
        self.get(key).map(|s| s.iter().copied().collect()).unwrap_or_default()
    }

    pub fn max(&self, key: &str) -> f32 {
        self.get(key).map(|s| s.iter().copied().fold(0.0, f32::max)).unwrap_or(0.0)
    }

    /// Forget series that weren't pushed to for a while (unplugged disks).
    pub fn retain(&mut self, mut keep: impl FnMut(&str) -> bool) {
        self.series.retain(|k, _| keep(k));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rolls_over() {
        let mut h = History::new(3);
        for v in [1.0, 2.0, 3.0, 4.0] {
            h.push("cpu", v);
        }
        assert_eq!(h.values("cpu"), vec![2.0, 3.0, 4.0]);
        assert_eq!(h.max("cpu"), 4.0);
        h.push("cpu", f32::NAN);
        assert_eq!(h.values("cpu"), vec![3.0, 4.0, 0.0]);
        h.set_len(2);
        assert_eq!(h.values("cpu"), vec![4.0, 0.0]);
        assert!(h.values("missing").is_empty());
        h.retain(|k| k != "cpu");
        assert!(h.get("cpu").is_none());
    }
}
