use super::RingBuffer;
use proptest::prelude::*;
use std::collections::VecDeque;

#[test]
fn starts_empty() {
    let rb = RingBuffer::new(8);
    assert_eq!(rb.len(), 0);
    assert!(rb.is_empty());
    assert_eq!(rb.snapshot(), Vec::<u8>::new());
}

#[test]
#[should_panic(expected = "power of two")]
fn rejects_non_power_of_two_capacity() {
    RingBuffer::new(6);
}

#[test]
fn append_under_capacity_keeps_everything_in_order() {
    let mut rb = RingBuffer::new(8);
    rb.append(b"AB");
    rb.append(b"CD");
    assert_eq!(rb.snapshot(), b"ABCD");
    assert_eq!(rb.len(), 4);
}

#[test]
fn append_exactly_filling_capacity_keeps_everything() {
    let mut rb = RingBuffer::new(4);
    rb.append(b"ABCD");
    assert_eq!(rb.snapshot(), b"ABCD");
    assert_eq!(rb.len(), 4);
}

#[test]
fn append_past_capacity_overwrites_oldest_bytes() {
    // Hand-traced case from the design: cap=4, "AB" then "CDE" should leave
    // "BCDE" (the last 4 bytes of "ABCDE" ever written).
    let mut rb = RingBuffer::new(4);
    rb.append(b"AB");
    rb.append(b"CDE");
    assert_eq!(rb.snapshot(), b"BCDE");
    assert_eq!(rb.len(), 4);
}

#[test]
fn append_wraps_correctly_across_multiple_overflowing_writes() {
    let mut rb = RingBuffer::new(4);
    rb.append(b"AB");
    rb.append(b"CDE"); // -> BCDE
    rb.append(b"F"); // -> CDEF
    assert_eq!(rb.snapshot(), b"CDEF");
}

#[test]
fn single_append_longer_than_capacity_keeps_only_the_tail() {
    let mut rb = RingBuffer::new(4);
    rb.append(b"ABCDEFGH");
    assert_eq!(rb.snapshot(), b"EFGH");
    assert_eq!(rb.len(), 4);
}

#[test]
fn appending_empty_slice_is_a_no_op() {
    let mut rb = RingBuffer::new(4);
    rb.append(b"AB");
    rb.append(b"");
    assert_eq!(rb.snapshot(), b"AB");
}

#[test]
fn many_small_appends_match_expected_tail() {
    let mut rb = RingBuffer::new(8);
    for byte in b'a'..=b'z' {
        rb.append(&[byte]);
    }
    // Last 8 letters of a..z: s,t,u,v,w,x,y,z
    assert_eq!(rb.snapshot(), b"stuvwxyz");
}

// Reference-model property test: a plain VecDeque that manually caps itself
// at `capacity` should agree with RingBuffer for any sequence of appends,
// including ones that overflow and re-overflow.
proptest! {
    #[test]
    fn matches_vecdeque_reference_model(
        chunks in proptest::collection::vec(
            proptest::collection::vec(any::<u8>(), 0..20),
            0..20,
        )
    ) {
        const CAP: usize = 16;
        let mut rb = RingBuffer::new(CAP);
        let mut reference: VecDeque<u8> = VecDeque::new();

        for chunk in &chunks {
            rb.append(chunk);
            for &b in chunk {
                if reference.len() == CAP {
                    reference.pop_front();
                }
                reference.push_back(b);
            }
        }

        let expected: Vec<u8> = reference.into_iter().collect();
        prop_assert_eq!(rb.snapshot(), expected);
    }
}
