//! `NixDeserialize` impls for tuples.
//!
//! Tuples are deserialized by reading each element in sequence (no length
//! prefix), matching the C++ `std::tuple` wire format.

use super::NixDeserialize;
use super::NixRead;

macro_rules! impl_nix_deserialize_tuple {
    ($($idx:tt $T:ident),+) => {
        impl<$($T),+> NixDeserialize for ($($T,)+)
        where
            $($T: NixDeserialize + Send + 'static,)+
        {
            async fn try_deserialize<R>(reader: &mut R) -> Result<Option<Self>, R::Error>
            where
                R: ?Sized + NixRead + Send,
            {
                impl_nix_deserialize_tuple!(@body reader, $($idx $T),+)
            }
        }
    };

    // Body: try first element for EOF detection, then read rest
    (@body $reader:ident, 0 $T0:ident) => {
        Ok($reader.try_read_value::<$T0>().await?.map(|v| (v,)))
    };

    (@body $reader:ident, 0 $T0:ident, $($idx:tt $T:ident),+) => {
        match $reader.try_read_value::<$T0>().await? {
            None => Ok(None),
            Some(v0) => {
                Ok(Some((
                    v0,
                    $($reader.read_value::<$T>().await?,)+
                )))
            }
        }
    };
}

impl_nix_deserialize_tuple!(0 T0, 1 T1);
impl_nix_deserialize_tuple!(0 T0, 1 T1, 2 T2);
impl_nix_deserialize_tuple!(0 T0, 1 T1, 2 T2, 3 T3);
impl_nix_deserialize_tuple!(0 T0, 1 T1, 2 T2, 3 T3, 4 T4);
impl_nix_deserialize_tuple!(0 T0, 1 T1, 2 T2, 3 T3, 4 T4, 5 T5);
