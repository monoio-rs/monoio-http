#![feature(generic_associated_types)]
#![feature(type_alias_impl_trait)]

pub trait ParamRef<T> {
    fn param_ref(&self) -> &T;
}

mod h1;
mod request;
mod response;
