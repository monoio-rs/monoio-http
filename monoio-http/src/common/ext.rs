use std::{borrow::Cow, ops::Deref};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Reason(Cow<'static, str>);

impl From<Cow<'static, str>> for Reason {
    fn from(inner: Cow<'static, str>) -> Self {
        Self(inner)
    }
}

impl Deref for Reason {
    type Target = Cow<'static, str>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
