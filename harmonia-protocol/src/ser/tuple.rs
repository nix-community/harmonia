//! `NixSerialize` impls for tuples.
//!
//! Tuples are serialized by writing each element in sequence (no length
//! prefix), matching the C++ `std::tuple` wire format.

use super::NixSerialize;
use super::NixWrite;

macro_rules! impl_nix_serialize_tuple {
    ($($idx:tt $T:ident),+) => {
        impl<$($T),+> NixSerialize for ($($T,)+)
        where
            $($T: NixSerialize + Send + Sync,)+
        {
            async fn serialize<W>(&self, writer: &mut W) -> Result<(), W::Error>
            where
                W: NixWrite,
            {
                $(writer.write_value(&self.$idx).await?;)+
                Ok(())
            }
        }
    };
}

impl_nix_serialize_tuple!(0 T0, 1 T1);
impl_nix_serialize_tuple!(0 T0, 1 T1, 2 T2);
impl_nix_serialize_tuple!(0 T0, 1 T1, 2 T2, 3 T3);
impl_nix_serialize_tuple!(0 T0, 1 T1, 2 T2, 3 T3, 4 T4);
impl_nix_serialize_tuple!(0 T0, 1 T1, 2 T2, 3 T3, 4 T4, 5 T5);
