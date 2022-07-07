#![feature(generic_associated_types)]
#![feature(type_alias_impl_trait)]

pub trait ParamRef<T> {
    fn param_ref(&self) -> &T;
}

pub trait ParamMut<T> {
    fn param_mut(&mut self) -> &mut T;
}

pub mod common;
pub mod h1;
