#![allow(stable_features)]
#![feature(type_alias_impl_trait)]

pub trait Param<T> {
    fn param(&self) -> T;
}

pub trait ParamRef<T> {
    fn param_ref(&self) -> &T;
}

pub trait ParamMut<T> {
    fn param_mut(&mut self) -> &mut T;
}

pub mod common;
pub mod h1;
pub mod util;
