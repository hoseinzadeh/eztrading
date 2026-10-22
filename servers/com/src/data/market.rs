use super::provider::Provider;
use std::marker::PhantomData;

pub struct Market<P: ?Sized> {
    name: String,
    code: String,
    country: String,
    is_open: bool,

    pub closing_time: Option<i64>,
    pub opening_time: Option<i64>,

    phantom: PhantomData<P>,
}

impl<P> std::fmt::Debug for Market<P> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Market")
            .field("name", &self.name)
            .field("code", &self.code)
            .field("country", &self.country)
            .field("is_open", &self.is_open)
            .field("closing_time", &self.closing_time)
            .field("opening_time", &self.opening_time)
            .finish()
    }
}

unsafe impl<P: ?Sized> Send for Market<P> {}
unsafe impl<P: ?Sized> Sync for Market<P> {}

impl<P: Provider> Market<P> {
    pub fn new(name: String, code: String, country: String) -> Self {
        Self {
            name,
            code,
            country,
            is_open: false,
            closing_time: None,
            opening_time: None,
            phantom: PhantomData,
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn code(&self) -> &str {
        &self.code
    }

    pub fn country(&self) -> &str {
        &self.country
    }

    pub fn is_open(&self) -> bool {
        self.is_open
    }

    pub fn set_open(&mut self, open: bool) {
        self.is_open = open
    }

    pub fn closing_time(&self) -> Option<i64> {
        self.closing_time
    }

    pub fn opening_time(&self) -> Option<i64> {
        self.opening_time
    }
}
