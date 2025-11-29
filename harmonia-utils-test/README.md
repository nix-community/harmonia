# harmonia-utils-test

Proptest strategies and test macros for property-based testing.

**Contents**:
- `arb_filename` / `arb_path` - Strategies for generating valid filenames and paths
- `arb_byte_string` - Strategy for generating arbitrary byte strings
- `arb_duration` / `arb_system_time` - Strategies for time values
- `pretty_prop_assert_eq!` - Assertion macro with pretty diff output
- `helpers::Union` - Weighted union of proptest strategies

**Example API**:
```rust
use harmonia_utils_test::{arb_path, pretty_prop_assert_eq};
use proptest::prelude::*;

proptest! {
    #[test]
    fn roundtrip(path in arb_path()) {
        let encoded = encode(&path);
        let decoded = decode(&encoded)?;
        pretty_prop_assert_eq!(path, decoded);
    }
}
```
