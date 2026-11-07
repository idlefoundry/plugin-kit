//! Serde helpers for the steppers' state (a built model is kept on disk by its instrument).

/// A fixed array longer than serde's 32, written as a sequence and read back at its length.
pub mod array {
    use serde::de::Error;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer, T: Serialize, const N: usize>(
        a: &[T; N],
        s: S,
    ) -> Result<S::Ok, S::Error> {
        a.as_slice().serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>, T: Deserialize<'de>, const N: usize>(
        d: D,
    ) -> Result<[T; N], D::Error> {
        let v = Vec::<T>::deserialize(d)?;
        let n = v.len();
        v.try_into()
            .map_err(|_| D::Error::invalid_length(n, &"the array's length"))
    }
}
