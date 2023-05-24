#![allow(stable_features)]
#![feature(generic_associated_types)]
#![feature(type_alias_impl_trait)]
#![feature(impl_trait_in_assoc_type)]

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
pub mod h2;
pub mod util;
