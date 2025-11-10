use quote::ToTokens;
use syn::meta::ParseNestedMeta;
use syn::parse::Parse;
use syn::punctuated::Punctuated;
use syn::{Attribute, Expr, ExprLit, ExprPath, Lit, Token, parse_quote, token};

use crate::internal::symbol::{BOUND, DESERIALIZE, SERIALIZE};

use super::Context;
use super::symbol::{
    CRATE, DEFAULT, DISPLAY, FROM, FROM_STORE_DIR_STR, FROM_STR, INTO, NIX, SKIP,
    STORE_DIR_DISPLAY, Symbol, TAG, TRY_FROM, TRY_INTO, VERSION,
};

#[derive(Debug, PartialEq)]
pub enum Default {
    None,
    #[allow(clippy::enum_variant_names)]
    Default(syn::Path),
    Path(ExprPath),
}

impl Default {
    pub fn is_none(&self) -> bool {
        matches!(self, Default::None)
    }
}

#[derive(Debug, PartialEq)]
pub struct Field {
    pub default: Default,
    pub version: Option<syn::ExprRange>,
    pub skip: bool,
}

impl Field {
    pub fn from_ast(ctx: &Context, attrs: &Vec<Attribute>) -> Field {
        let mut version = None;
        let mut version_path = None;
        let mut default = Default::None;
        let mut skip = false;
        for attr in attrs {
            if attr.path() != NIX {
                continue;
            }
            if let Err(err) = attr.parse_nested_meta(|meta| {
                if meta.path == VERSION {
                    version = parse_lit(ctx, &meta, VERSION)?;
                    version_path = Some(meta.path);
                } else if meta.path == DEFAULT {
                    if meta.input.peek(Token![=]) {
                        if let Some(path) = parse_lit(ctx, &meta, DEFAULT)? {
                            default = Default::Path(path);
                        }
                    } else {
                        default = Default::Default(meta.path);
                    }
                } else if meta.path == SKIP {
                    skip = true;
                } else {
                    let path = meta.path.to_token_stream().to_string();
                    return Err(meta.error(format_args!("unknown nix field attribute '{path}'")));
                }
                Ok(())
            }) {
                eprintln!("{:?}", err.span().source_text());
                ctx.syn_error(err);
            }
        }
        if version.is_some() && default.is_none() {
            default = Default::Default(version_path.unwrap());
        }

        Field {
            default,
            version,
            skip,
        }
    }
}

#[derive(Debug, PartialEq)]
pub struct Variant {
    pub version: syn::ExprRange,
    pub tag: Option<syn::Ident>,
}

impl Variant {
    pub fn from_ast(ctx: &Context, attrs: &Vec<Attribute>) -> Variant {
        let mut version = parse_quote!(..);
        let mut tag = None;
        for attr in attrs {
            if attr.path() != NIX {
                continue;
            }
            if let Err(err) = attr.parse_nested_meta(|meta| {
                if meta.path == VERSION {
                    if let Some(v) = parse_lit(ctx, &meta, VERSION)? {
                        version = v;
                    }
                } else if meta.path == TAG {
                    tag = parse_lit(ctx, &meta, TAG)?;
                } else {
                    let path = meta.path.to_token_stream().to_string();
                    return Err(meta.error(format_args!("unknown nix variant attribute '{path}'")));
                }
                Ok(())
            }) {
                eprintln!("{:?}", err.span().source_text());
                ctx.syn_error(err);
            }
        }

        Variant { version, tag }
    }
}

#[derive(Debug, PartialEq)]
pub struct Container {
    pub from_str: Option<syn::Path>,
    pub from_store_dir_str: Option<syn::Path>,
    pub type_from: Option<syn::Type>,
    pub type_try_from: Option<syn::Type>,
    pub type_into: Option<syn::Type>,
    pub type_try_into: Option<syn::Type>,
    pub display: Default,
    pub store_dir_display: Option<syn::Path>,
    pub crate_path: Option<syn::Path>,
    pub tag: Option<syn::Type>,
    pub ser_bound: Option<Vec<syn::WherePredicate>>,
    pub de_bound: Option<Vec<syn::WherePredicate>>,
}

impl Container {
    pub fn from_ast(ctx: &Context, attrs: &Vec<Attribute>) -> Container {
        let mut type_from = None;
        let mut type_try_from = None;
        let mut crate_path = None;
        let mut from_str = None;
        let mut from_store_dir_str = None;
        let mut type_into = None;
        let mut type_try_into = None;
        let mut display = Default::None;
        let mut store_dir_display = None;
        let mut tag = None;
        let mut ser_bound = None;
        let mut de_bound = None;

        for attr in attrs {
            if attr.path() != NIX {
                continue;
            }
            if let Err(err) = attr.parse_nested_meta(|meta| {
                if meta.path == FROM {
                    type_from = parse_lit(ctx, &meta, FROM)?;
                } else if meta.path == TRY_FROM {
                    type_try_from = parse_lit(ctx, &meta, TRY_FROM)?;
                } else if meta.path == FROM_STR {
                    from_str = Some(meta.path);
                } else if meta.path == FROM_STORE_DIR_STR {
                    from_store_dir_str = Some(meta.path);
                } else if meta.path == INTO {
                    type_into = parse_lit(ctx, &meta, INTO)?;
                } else if meta.path == TRY_INTO {
                    type_try_into = parse_lit(ctx, &meta, TRY_INTO)?;
                } else if meta.path == DISPLAY {
                    if meta.input.peek(Token![=]) {
                        if let Some(path) = parse_lit(ctx, &meta, DISPLAY)? {
                            display = Default::Path(path);
                        }
                    } else {
                        display = Default::Default(meta.path);
                    }
                } else if meta.path == STORE_DIR_DISPLAY {
                    store_dir_display = Some(meta.path);
                } else if meta.path == TAG {
                    tag = parse_lit(ctx, &meta, TAG)?;
                } else if meta.path == CRATE {
                    crate_path = parse_lit(ctx, &meta, CRATE)?;
                } else if meta.path == BOUND {
                    (ser_bound, de_bound) =
                        get_ser_and_de(ctx, BOUND, &meta, parse_lit_into_where)?;
                } else {
                    let path = meta.path.to_token_stream().to_string();
                    return Err(
                        meta.error(format_args!("unknown nix container attribute '{path}'"))
                    );
                }
                Ok(())
            }) {
                eprintln!("{:?}", err.span().source_text());
                ctx.syn_error(err);
            }
        }

        Container {
            from_str,
            from_store_dir_str,
            type_from,
            type_try_from,
            type_into,
            type_try_into,
            display,
            store_dir_display,
            crate_path,
            tag,
            ser_bound,
            de_bound,
        }
    }
}

fn get_ser_and_de<T, F, R>(
    ctx: &Context,
    attr_name: Symbol,
    meta: &ParseNestedMeta,
    f: F,
) -> syn::Result<(Option<T>, Option<T>)>
where
    T: Clone,
    F: Fn(&Context, Symbol, Symbol, &ParseNestedMeta) -> syn::Result<R>,
    R: Into<Option<T>>,
{
    let mut ser_meta = None;
    let mut de_meta = None;

    let lookahead = meta.input.lookahead1();
    if lookahead.peek(Token![=]) {
        if let Some(both) = f(ctx, attr_name, attr_name, meta)?.into() {
            ser_meta = Some(both.clone());
            de_meta = Some(both);
        }
    } else if lookahead.peek(token::Paren) {
        meta.parse_nested_meta(|meta| {
            if meta.path == SERIALIZE {
                if let Some(v) = f(ctx, attr_name, SERIALIZE, &meta)?.into() {
                    if ser_meta.is_some() {
                        ctx.error_spanned(
                            meta.path,
                            format_args!("duplicate nix attribute '{SERIALIZE}'"),
                        );
                    } else {
                        ser_meta = Some(v);
                    }
                }
            } else if meta.path == DESERIALIZE {
                if let Some(v) = f(ctx, attr_name, DESERIALIZE, &meta)?.into() {
                    if de_meta.is_some() {
                        ctx.error_spanned(
                            meta.path,
                            format_args!("duplicate nix attribute '{DESERIALIZE}'"),
                        );
                    } else {
                        de_meta = Some(v);
                    }
                }
            } else {
                return Err(meta.error(format_args!(
                    "malformed {attr_name} attribute, expected `{attr_name}(serialize = ..., deserialize = ...)`",
                )));
            }
            Ok(())
        })?;
    } else {
        return Err(lookahead.error());
    }

    Ok((ser_meta, de_meta))
}

pub fn get_lit_str(
    ctx: &Context,
    meta: &ParseNestedMeta,
    attr: Symbol,
) -> syn::Result<Option<syn::LitStr>> {
    let expr: Expr = meta.value()?.parse()?;
    let mut value = &expr;
    while let Expr::Group(e) = value {
        value = &e.expr;
    }
    if let Expr::Lit(ExprLit {
        lit: Lit::Str(s), ..
    }) = value
    {
        Ok(Some(s.clone()))
    } else {
        ctx.error_spanned(
            expr,
            format_args!("expected nix attribute {attr} to be string"),
        );
        Ok(None)
    }
}

fn get_lit_str2(
    ctx: &Context,
    attr_name: Symbol,
    meta_item_name: Symbol,
    meta: &ParseNestedMeta,
) -> syn::Result<Option<syn::LitStr>> {
    let expr: syn::Expr = meta.value()?.parse()?;
    let mut value = &expr;
    while let syn::Expr::Group(e) = value {
        value = &e.expr;
    }
    if let syn::Expr::Lit(syn::ExprLit {
        lit: syn::Lit::Str(lit),
        ..
    }) = value
    {
        let suffix = lit.suffix();
        if !suffix.is_empty() {
            ctx.error_spanned(
                lit,
                format!("unexpected suffix `{suffix}` on string literal"),
            );
        }
        Ok(Some(lit.clone()))
    } else {
        ctx.error_spanned(
            expr,
            format!(
                "expected serde {attr_name} attribute to be a string: `{meta_item_name} = \"...\"`",
            ),
        );
        Ok(None)
    }
}

fn parse_lit_into_where(
    cx: &Context,
    attr_name: Symbol,
    meta_item_name: Symbol,
    meta: &ParseNestedMeta,
) -> syn::Result<Vec<syn::WherePredicate>> {
    let string = match get_lit_str2(cx, attr_name, meta_item_name, meta)? {
        Some(string) => string,
        None => return Ok(Vec::new()),
    };

    Ok(
        match string.parse_with(Punctuated::<syn::WherePredicate, Token![,]>::parse_terminated) {
            Ok(predicates) => Vec::from_iter(predicates),
            Err(err) => {
                cx.error_spanned(string, err);
                Vec::new()
            }
        },
    )
}

pub fn parse_lit<T: Parse>(
    ctx: &Context,
    meta: &ParseNestedMeta,
    attr: Symbol,
) -> syn::Result<Option<T>> {
    match get_lit_str(ctx, meta, attr)? {
        Some(lit) => Ok(Some(lit.parse()?)),
        None => Ok(None),
    }
}

#[cfg(test)]
mod test {
    use syn::{Attribute, parse_quote};

    use crate::internal::Context;

    use super::*;

    #[test]
    fn parse_field_version() {
        let attrs: Vec<Attribute> = vec![parse_quote!(#[nix(version="..34")])];
        let ctx = Context::new();
        let field = Field::from_ast(&ctx, &attrs);
        ctx.check().unwrap();
        assert_eq!(
            field,
            Field {
                default: Default::Default(parse_quote!(version)),
                version: Some(parse_quote!(..34)),
                skip: false,
            }
        );
    }

    #[test]
    fn parse_field_default() {
        let attrs: Vec<Attribute> = vec![parse_quote!(#[nix(default)])];
        let ctx = Context::new();
        let field = Field::from_ast(&ctx, &attrs);
        ctx.check().unwrap();
        assert_eq!(
            field,
            Field {
                default: Default::Default(parse_quote!(default)),
                version: None,
                skip: false,
            }
        );
    }

    #[test]
    fn parse_field_default_path() {
        let attrs: Vec<Attribute> = vec![parse_quote!(#[nix(default="Default::default")])];
        let ctx = Context::new();
        let field = Field::from_ast(&ctx, &attrs);
        ctx.check().unwrap();
        assert_eq!(
            field,
            Field {
                default: Default::Path(parse_quote!(Default::default)),
                version: None,
                skip: false,
            }
        );
    }

    #[test]
    fn parse_field_both() {
        let attrs: Vec<Attribute> =
            vec![parse_quote!(#[nix(version="..", default="Default::default")])];
        let ctx = Context::new();
        let field = Field::from_ast(&ctx, &attrs);
        ctx.check().unwrap();
        assert_eq!(
            field,
            Field {
                default: Default::Path(parse_quote!(Default::default)),
                version: Some(parse_quote!(..)),
                skip: false,
            }
        );
    }

    #[test]
    fn parse_field_both_rev() {
        let attrs: Vec<Attribute> =
            vec![parse_quote!(#[nix(default="Default::default", version="..")])];
        let ctx = Context::new();
        let field = Field::from_ast(&ctx, &attrs);
        ctx.check().unwrap();
        assert_eq!(
            field,
            Field {
                default: Default::Path(parse_quote!(Default::default)),
                version: Some(parse_quote!(..)),
                skip: false,
            }
        );
    }

    #[test]
    fn parse_field_no_attr() {
        let attrs: Vec<Attribute> = vec![];
        let ctx = Context::new();
        let field = Field::from_ast(&ctx, &attrs);
        ctx.check().unwrap();
        assert_eq!(
            field,
            Field {
                default: Default::None,
                version: None,
                skip: false,
            }
        );
    }

    #[test]
    fn parse_field_no_subattrs() {
        let attrs: Vec<Attribute> = vec![parse_quote!(#[nix()])];
        let ctx = Context::new();
        let field = Field::from_ast(&ctx, &attrs);
        ctx.check().unwrap();
        assert_eq!(
            field,
            Field {
                default: Default::None,
                version: None,
                skip: false,
            }
        );
    }

    #[test]
    fn parse_variant_version() {
        let attrs: Vec<Attribute> = vec![parse_quote!(#[nix(version="..34")])];
        let ctx = Context::new();
        let variant = Variant::from_ast(&ctx, &attrs);
        ctx.check().unwrap();
        assert_eq!(
            variant,
            Variant {
                version: parse_quote!(..34),
                tag: None,
            }
        );
    }

    #[test]
    fn parse_variant_no_attr() {
        let attrs: Vec<Attribute> = vec![];
        let ctx = Context::new();
        let variant = Variant::from_ast(&ctx, &attrs);
        ctx.check().unwrap();
        assert_eq!(
            variant,
            Variant {
                version: parse_quote!(..),
                tag: None,
            }
        );
    }

    #[test]
    fn parse_variant_no_subattrs() {
        let attrs: Vec<Attribute> = vec![parse_quote!(#[nix()])];
        let ctx = Context::new();
        let variant = Variant::from_ast(&ctx, &attrs);
        ctx.check().unwrap();
        assert_eq!(
            variant,
            Variant {
                version: parse_quote!(..),
                tag: None,
            }
        );
    }

    #[test]
    fn parse_variant_tag() {
        let attrs: Vec<Attribute> = vec![parse_quote!(#[nix(tag="Testing")])];
        let ctx = Context::new();
        let variant = Variant::from_ast(&ctx, &attrs);
        ctx.check().unwrap();
        assert_eq!(
            variant,
            Variant {
                version: parse_quote!(..),
                tag: Some(parse_quote!(Testing)),
            }
        );
    }

    #[test]
    fn parse_container_from_str() {
        let attrs: Vec<Attribute> = vec![parse_quote!(#[nix(from_str)])];
        let ctx = Context::new();
        let container = Container::from_ast(&ctx, &attrs);
        ctx.check().unwrap();
        assert_eq!(
            container,
            Container {
                from_str: Some(parse_quote!(from_str)),
                from_store_dir_str: None,
                type_from: None,
                type_try_from: None,
                type_into: None,
                type_try_into: None,
                display: Default::None,
                store_dir_display: None,
                crate_path: None,
                tag: None,
                ser_bound: None,
                de_bound: None,
            }
        );
    }

    #[test]
    fn parse_container_from_store_dir_str() {
        let attrs: Vec<Attribute> = vec![parse_quote!(#[nix(from_store_dir_str)])];
        let ctx = Context::new();
        let container = Container::from_ast(&ctx, &attrs);
        ctx.check().unwrap();
        assert_eq!(
            container,
            Container {
                from_str: None,
                from_store_dir_str: Some(parse_quote!(from_store_dir_str)),
                type_from: None,
                type_try_from: None,
                type_into: None,
                type_try_into: None,
                display: Default::None,
                store_dir_display: None,
                crate_path: None,
                tag: None,
                ser_bound: None,
                de_bound: None,
            }
        );
    }

    #[test]
    fn parse_container_from() {
        let attrs: Vec<Attribute> = vec![parse_quote!(#[nix(from="u64")])];
        let ctx = Context::new();
        let container = Container::from_ast(&ctx, &attrs);
        ctx.check().unwrap();
        assert_eq!(
            container,
            Container {
                from_str: None,
                from_store_dir_str: None,
                type_from: Some(parse_quote!(u64)),
                type_try_from: None,
                type_into: None,
                type_try_into: None,
                display: Default::None,
                store_dir_display: None,
                crate_path: None,
                tag: None,
                ser_bound: None,
                de_bound: None,
            }
        );
    }

    #[test]
    fn parse_container_try_from() {
        let attrs: Vec<Attribute> = vec![parse_quote!(#[nix(try_from="u64")])];
        let ctx = Context::new();
        let container = Container::from_ast(&ctx, &attrs);
        ctx.check().unwrap();
        assert_eq!(
            container,
            Container {
                from_str: None,
                from_store_dir_str: None,
                type_from: None,
                type_try_from: Some(parse_quote!(u64)),
                type_into: None,
                type_try_into: None,
                display: Default::None,
                store_dir_display: None,
                crate_path: None,
                tag: None,
                ser_bound: None,
                de_bound: None,
            }
        );
    }

    #[test]
    fn parse_container_into() {
        let attrs: Vec<Attribute> = vec![parse_quote!(#[nix(into="u64")])];
        let ctx = Context::new();
        let container = Container::from_ast(&ctx, &attrs);
        ctx.check().unwrap();
        assert_eq!(
            container,
            Container {
                from_str: None,
                from_store_dir_str: None,
                type_from: None,
                type_try_from: None,
                type_into: Some(parse_quote!(u64)),
                type_try_into: None,
                display: Default::None,
                store_dir_display: None,
                crate_path: None,
                tag: None,
                ser_bound: None,
                de_bound: None,
            }
        );
    }

    #[test]
    fn parse_container_try_into() {
        let attrs: Vec<Attribute> = vec![parse_quote!(#[nix(try_into="u64")])];
        let ctx = Context::new();
        let container = Container::from_ast(&ctx, &attrs);
        ctx.check().unwrap();
        assert_eq!(
            container,
            Container {
                from_str: None,
                from_store_dir_str: None,
                type_from: None,
                type_try_from: None,
                type_into: None,
                type_try_into: Some(parse_quote!(u64)),
                display: Default::None,
                store_dir_display: None,
                crate_path: None,
                tag: None,
                ser_bound: None,
                de_bound: None,
            }
        );
    }

    #[test]
    fn parse_container_display() {
        let attrs: Vec<Attribute> = vec![parse_quote!(#[nix(display)])];
        let ctx = Context::new();
        let container = Container::from_ast(&ctx, &attrs);
        ctx.check().unwrap();
        assert_eq!(
            container,
            Container {
                from_str: None,
                from_store_dir_str: None,
                type_from: None,
                type_try_from: None,
                type_into: None,
                type_try_into: None,
                display: Default::Default(parse_quote!(display)),
                store_dir_display: None,
                crate_path: None,
                tag: None,
                ser_bound: None,
                de_bound: None,
            }
        );
    }

    #[test]
    fn parse_container_display_path() {
        let attrs: Vec<Attribute> = vec![parse_quote!(#[nix(display="Path::display")])];
        let ctx = Context::new();
        let container = Container::from_ast(&ctx, &attrs);
        ctx.check().unwrap();
        assert_eq!(
            container,
            Container {
                from_str: None,
                from_store_dir_str: None,
                type_from: None,
                type_try_from: None,
                type_into: None,
                type_try_into: None,
                display: Default::Path(parse_quote!(Path::display)),
                store_dir_display: None,
                crate_path: None,
                tag: None,
                ser_bound: None,
                de_bound: None,
            }
        );
    }

    #[test]
    fn parse_container_store_dir_display() {
        let attrs: Vec<Attribute> = vec![parse_quote!(#[nix(store_dir_display)])];
        let ctx = Context::new();
        let container = Container::from_ast(&ctx, &attrs);
        ctx.check().unwrap();
        assert_eq!(
            container,
            Container {
                from_str: None,
                from_store_dir_str: None,
                type_from: None,
                type_try_from: None,
                type_into: None,
                type_try_into: None,
                display: Default::None,
                store_dir_display: Some(parse_quote!(store_dir_display)),
                crate_path: None,
                tag: None,
                ser_bound: None,
                de_bound: None,
            }
        );
    }

    #[test]
    fn parse_container_tag() {
        let attrs: Vec<Attribute> = vec![parse_quote!(#[nix(tag="::test::Operation")])];
        let ctx = Context::new();
        let container = Container::from_ast(&ctx, &attrs);
        ctx.check().unwrap();
        assert_eq!(
            container,
            Container {
                from_str: None,
                from_store_dir_str: None,
                type_from: None,
                type_try_from: None,
                type_into: None,
                type_try_into: None,
                display: Default::None,
                store_dir_display: None,
                crate_path: None,
                tag: Some(parse_quote!(::test::Operation)),
                ser_bound: None,
                de_bound: None,
            }
        );
    }
}
