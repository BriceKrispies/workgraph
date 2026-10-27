/// Declares a fieldless enum whose wire form is a fixed string per variant.
///
/// Generates `ALL`, `as_str`, `parse`, `Display`, `FromStr` (error: `validation_failed`) and
/// serde string (de)serialisation.
macro_rules! string_enum {
    (
        $(#[$meta:meta])*
        $vis:vis enum $name:ident { $( $variant:ident = $text:literal ),+ $(,)? }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        $vis enum $name {
            $( $variant ),+
        }

        impl $name {
            /// Every variant, in declaration order.
            pub const ALL: &'static [Self] = &[ $( Self::$variant ),+ ];

            /// The wire string of this variant.
            pub const fn as_str(self) -> &'static str {
                match self {
                    $( Self::$variant => $text ),+
                }
            }

            /// Parses a wire string; `None` when it names no variant.
            pub fn parse(text: &str) -> Option<Self> {
                match text {
                    $( $text => Some(Self::$variant), )+
                    _ => None,
                }
            }
        }

        impl core::fmt::Display for $name {
            fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl core::str::FromStr for $name {
            type Err = $crate::WgError;

            fn from_str(text: &str) -> Result<Self, Self::Err> {
                Self::parse(text).ok_or_else(|| {
                    $crate::WgError::validation(format!(
                        "unknown {} {text:?}",
                        stringify!($name)
                    ))
                })
            }
        }

        impl serde::Serialize for $name {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_str(self.as_str())
            }
        }

        impl<'de> serde::Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let text = <String as serde::Deserialize>::deserialize(deserializer)?;
                Self::parse(&text).ok_or_else(|| {
                    serde::de::Error::custom(format!("unknown {} {text:?}", stringify!($name)))
                })
            }
        }
    };
}
