#![feature(generic_associated_types)]
#![feature(type_alias_impl_trait)]

pub trait ParamRef<T> {
    fn param_ref(&self) -> &T;
}

pub mod h1;
pub mod common;
