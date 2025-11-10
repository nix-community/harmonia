use std::fmt;

use syn::Path;

#[derive(Copy, Clone)]
pub struct Symbol(&'static str);

pub const NIX: Symbol = Symbol("nix");
pub const VERSION: Symbol = Symbol("version");
pub const DEFAULT: Symbol = Symbol("default");
pub const FROM: Symbol = Symbol("from");
pub const TRY_FROM: Symbol = Symbol("try_from");
pub const FROM_STR: Symbol = Symbol("from_str");
pub const FROM_STORE_DIR_STR: Symbol = Symbol("from_store_dir_str");
pub const INTO: Symbol = Symbol("into");
pub const TRY_INTO: Symbol = Symbol("try_into");
pub const DISPLAY: Symbol = Symbol("display");
pub const SKIP: Symbol = Symbol("skip");
pub const STORE_DIR_DISPLAY: Symbol = Symbol("store_dir_display");
pub const CRATE: Symbol = Symbol("crate");
pub const TAG: Symbol = Symbol("tag");
pub const BOUND: Symbol = Symbol("bound");
pub const SERIALIZE: Symbol = Symbol("serialize");
pub const DESERIALIZE: Symbol = Symbol("deserialize");

impl PartialEq<Symbol> for Path {
    fn eq(&self, word: &Symbol) -> bool {
        self.is_ident(word.0)
    }
}

impl PartialEq<Symbol> for &Path {
    fn eq(&self, word: &Symbol) -> bool {
        self.is_ident(word.0)
    }
}

impl fmt::Display for Symbol {
    fn fmt(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str(self.0)
    }
}
