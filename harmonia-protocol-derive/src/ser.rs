use proc_macro2::{Span, TokenStream};
use quote::{quote, quote_spanned};
use syn::spanned::Spanned;
use syn::{DeriveInput, Generics, Path, Type};

use crate::internal::attrs::Default;
use crate::internal::inputs::RemoteInput;
use crate::internal::{Container, Context, Data, Field, Remote, Style, Variant, attrs};

pub fn expand_nix_serialize(crate_path: Path, input: &mut DeriveInput) -> syn::Result<TokenStream> {
    let cx = Context::new();
    let cont = Container::from_ast(&cx, crate_path, input);
    cx.check()?;
    let cont = cont.unwrap();

    let ty = cont.ident_type();
    let body = nix_serialize_body(&cont);
    let crate_path = cont.crate_path();

    Ok(nix_serialize_impl(
        crate_path,
        &ty,
        &cont.attrs,
        &cont.original.generics,
        body,
    ))
}

pub fn expand_nix_serialize_remote(
    crate_path: Path,
    input: &RemoteInput,
) -> syn::Result<TokenStream> {
    let cx = Context::new();
    let remote = Remote::from_ast(&cx, crate_path, input);
    if let Some(attrs) = remote.as_ref().map(|r| &r.attrs)
        && attrs.display.is_none()
        && attrs.store_dir_display.is_none()
        && attrs.type_into.is_none()
        && attrs.type_try_into.is_none()
        && attrs.tag.is_none()
    {
        cx.error_spanned(
            input,
            "Missing into, try_into, display, store_dir_display or tag attribute",
        );
    }
    cx.check()?;
    let remote = remote.unwrap();

    let crate_path = remote.crate_path();
    let body = nix_serialize_body_into(crate_path, &remote.attrs).expect("From tokenstream");
    let generics = Generics::default();
    Ok(nix_serialize_impl(
        crate_path,
        remote.ty,
        &remote.attrs,
        &generics,
        body,
    ))
}

fn nix_serialize_impl(
    crate_path: &Path,
    ty: &Type,
    attrs: &attrs::Container,
    generics: &Generics,
    body: TokenStream,
) -> TokenStream {
    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();
    let where_clause = match (where_clause, &attrs.ser_bound) {
        (None, None) => None,
        (None, Some(bound)) => {
            let mut where_bound = syn::WhereClause {
                where_token: <syn::Token![where]>::default(),
                predicates: syn::punctuated::Punctuated::new(),
            };
            where_bound.predicates.extend(bound.iter().cloned());
            Some(where_bound)
        }
        (Some(generics_where), None) => Some(generics_where.clone()),
        (Some(generics_where), Some(bound)) => {
            let mut where_bound = generics_where.clone();
            where_bound.predicates.extend(bound.iter().cloned());
            Some(where_bound)
        }
    };
    quote! {
        #[automatically_derived]
        impl #impl_generics #crate_path::daemon::ser::NixSerialize for #ty #ty_generics
            #where_clause
        {
            async fn serialize<W>(&self, writer: &mut W) -> std::result::Result<(), W::Error>
                where W: #crate_path::daemon::ser::NixWrite
            {
                use #crate_path::daemon::ser::Error as _;
                #body
            }
        }
    }
}

fn nix_serialize_body_into(
    crate_path: &syn::Path,
    attrs: &attrs::Container,
) -> Option<TokenStream> {
    if let Default::Default(span) = &attrs.display {
        Some(nix_serialize_display(span.span()))
    } else if let Default::Path(path) = &attrs.display {
        Some(nix_serialize_display_path(path))
    } else if let Some(span) = attrs.store_dir_display.as_ref() {
        Some(nix_serialize_store_dir_display(span.span()))
    } else if let Some(type_into) = attrs.type_into.as_ref() {
        Some(nix_serialize_into(type_into))
    } else {
        attrs
            .type_try_into
            .as_ref()
            .map(|type_try_into| nix_serialize_try_into(crate_path, type_try_into))
    }
}

fn nix_serialize_body(cont: &Container) -> TokenStream {
    if let Some(tokens) = nix_serialize_body_into(cont.crate_path(), &cont.attrs) {
        tokens
    } else {
        match &cont.data {
            Data::Struct(_style, fields) => nix_serialize_struct(fields),
            Data::Enum(variants) => {
                if let Some(tag) = cont.attrs.tag.as_ref() {
                    nix_serialize_tagged_enum(tag, variants)
                } else {
                    nix_serialize_enum(variants)
                }
            }
        }
    }
}

fn nix_serialize_struct(fields: &[Field<'_>]) -> TokenStream {
    let write_fields = fields.iter().map(|f| {
        let field = &f.member;
        let ty = f.ty;
        let write_value = quote_spanned! {
            ty.span()=> writer.write_value(&self.#field).await?
        };
        if let Some(version) = f.attrs.version.as_ref() {
            quote! {
                if (#version).contains(&writer.version().minor()) {
                    #write_value;
                }
            }
        } else {
            quote! {
                #write_value;
            }
        }
    });

    quote! {
        #(#write_fields)*
        Ok(())
    }
}

fn nix_serialize_variant_fields(variant: &Variant<'_>) -> TokenStream {
    let write_fields = variant.fields.iter().map(|f| {
        let field = f.var_ident();
        let ty = f.ty;
        let write_value = quote_spanned! {
            ty.span()=> writer.write_value(#field).await?
        };
        if let Some(version) = f.attrs.version.as_ref() {
            quote! {
                if (#version).contains(&writer.version().minor()) {
                    #write_value;
                }
            }
        } else {
            quote! {
                #write_value;
            }
        }
    });
    quote! {
        #(#write_fields)*
    }
}

fn nix_serialize_variant_destructure(variant: &Variant<'_>) -> TokenStream {
    let ident = variant.ident;
    let field_names = variant.fields.iter().map(|f| f.var_ident());
    match variant.style {
        Style::Struct => {
            quote! {
                Self::#ident { #(#field_names),* }
            }
        }
        Style::Tuple => {
            quote! {
                Self::#ident(#(#field_names),*)
            }
        }
        Style::Unit => quote!(Self::#ident),
    }
}

fn nix_serialize_tagged_variant(tag: &Type, variant: &Variant<'_>) -> TokenStream {
    let write_fields = nix_serialize_variant_fields(variant);
    let destructure = nix_serialize_variant_destructure(variant);
    let tag_ident = variant.tag_ident();
    quote! {
        #destructure => {
            writer.write_value(&#tag::#tag_ident).await?;
            #write_fields
        }
    }
}

fn nix_serialize_tagged_enum(tag: &Type, variants: &[Variant<'_>]) -> TokenStream {
    let match_variant = variants
        .iter()
        .map(|variant| nix_serialize_tagged_variant(tag, variant));
    quote! {
        match self {
            #(#match_variant)*
        }
        Ok(())
    }
}

fn nix_serialize_variant(variant: &Variant<'_>) -> TokenStream {
    let ident = variant.ident;
    let write_fields = nix_serialize_variant_fields(variant);
    let destructure = nix_serialize_variant_destructure(variant);
    let ignore = match variant.style {
        Style::Struct => {
            quote! {
                Self::#ident { .. }
            }
        }
        Style::Tuple => {
            quote! {
                Self::#ident(_, ..)
            }
        }
        Style::Unit => quote!(Self::#ident),
    };
    let version = &variant.attrs.version;
    quote! {
        #destructure if (#version).contains(&writer.version().minor()) => {
            #write_fields
        }
        #ignore => {
            return Err(W::Error::invalid_enum(format!("{} is not valid for version {}", "#ident", writer.version())));
        }
    }
}

fn nix_serialize_enum(variants: &[Variant<'_>]) -> TokenStream {
    let match_variant = variants
        .iter()
        .map(|variant| nix_serialize_variant(variant));
    quote! {
        match self {
            #(#match_variant)*
        }
        Ok(())
    }
}

fn nix_serialize_into(ty: &Type) -> TokenStream {
    quote_spanned! {
        ty.span() =>
        {
            let other : #ty = <Self as Clone>::clone(self).into();
            writer.write_value(&other).await
        }
    }
}

fn nix_serialize_try_into(crate_path: &Path, ty: &Type) -> TokenStream {
    quote_spanned! {
        ty.span() =>
        {
            use #crate_path::daemon::ser::Error;
            let other : #ty = <Self as Clone>::clone(self).try_into().map_err(Error::unsupported_data)?;
            writer.write_value(&other).await
        }
    }
}

fn nix_serialize_display(span: Span) -> TokenStream {
    quote_spanned! {
        span => writer.write_display(self).await
    }
}

fn nix_serialize_display_path(path: &syn::ExprPath) -> TokenStream {
    quote_spanned! {
        path.span() => writer.write_display(#path(self)).await
    }
}

fn nix_serialize_store_dir_display(span: Span) -> TokenStream {
    quote_spanned! {
        span =>
        {
            let store_dir = writer.store_dir().clone();
            writer.write_display(store_dir.display(self)).await
        }
    }
}
