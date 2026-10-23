#![no_main]

use libfuzzer_sys::fuzz_target;
use rarx_filters::{Delta, Filter};

fuzz_target!(|input: (u8, Vec<u8>)| {
    let (distance, original) = input;
    let distance = usize::from(distance) + 1;
    let mut data = original.clone();
    Delta::new(distance).unwrap().encode(&mut data);
    Delta::new(distance).unwrap().decode(&mut data);
    assert_eq!(data, original);
});
