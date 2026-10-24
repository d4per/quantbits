//! Procedural macros for `quantbits`. Use them via `quantbits::bitfields` and
//! `quantbits::BitEnum`, not directly.

use std::collections::HashMap;

use proc_macro::TokenStream;
use proc_macro2::{Span, TokenStream as TokenStream2};
use quote::{format_ident, quote, quote_spanned, ToTokens};
use syn::{
    ext::IdentExt,
    parse::{Parse, ParseStream},
    parse_macro_input,
    punctuated::Punctuated,
    spanned::Spanned,
    Attribute, Data, DeriveInput, Error, Expr, ExprRange, Fields, Ident, ItemStruct, LitInt, Path,
    RangeLimits, Result, Token, Type, Visibility,
};

#[proc_macro_attribute]
pub fn bitfields(args: TokenStream, input: TokenStream) -> TokenStream {
    let args = TokenStream2::from(args);
    if !args.is_empty() {
        return Error::new_spanned(args, "`#[bitfields]` takes no arguments").to_compile_error().into();
    }
    let item = parse_macro_input!(input as ItemStruct);
    match expand(item) {
        Ok(ts) => ts.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

#[proc_macro_derive(BitEnum, attributes(fallback))]
pub fn derive_bit_enum(input: TokenStream) -> TokenStream {
    let item = parse_macro_input!(input as DeriveInput);
    match expand_bit_enum(item) {
        Ok(ts) => ts.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

fn combine(acc: &mut Option<Error>, e: Error) {
    match acc {
        Some(acc) => acc.combine(e),
        None => *acc = Some(e),
    }
}

/// "1 bit" or "N bits".
fn n_bits(n: impl Into<u128>) -> String {
    match n.into() {
        1 => "1 bit".to_string(),
        n => format!("{n} bits"),
    }
}

/// "bit 5" or "bits 5–9".
fn bit_span(offset: usize, bits: u32) -> String {
    match bits {
        1 => format!("bit {offset}"),
        _ => format!("bits {offset}–{}", offset + bits as usize - 1),
    }
}

// ---------------------------------------------------------------------------
// Parsing of `#[bits(N)]` and `#[bits(N, range = MIN..MAX, wrap)]`
// ---------------------------------------------------------------------------

struct BitsArgs {
    bits: LitInt,
    range: Option<ExprRange>,
    wrap: Option<Ident>,
}

impl Parse for BitsArgs {
    fn parse(input: ParseStream) -> Result<Self> {
        let bits: LitInt = input.parse()?;
        let mut range = None;
        let mut wrap = None;
        while !input.is_empty() {
            input.parse::<Token![,]>()?;
            if input.is_empty() {
                break;
            }
            let key: Ident = input.parse()?;
            if key == "range" {
                if range.is_some() {
                    return Err(Error::new(key.span(), "`range` may only be given once"));
                }
                input.parse::<Token![=]>()?;
                match input.parse::<Expr>()? {
                    Expr::Range(r) => range = Some(r),
                    other => {
                        return Err(Error::new_spanned(
                            other,
                            "expected a range, e.g. `0.0..1.0` or `-1.0..=1.0`",
                        ))
                    }
                }
            } else if key == "wrap" {
                if wrap.is_some() {
                    return Err(Error::new(key.span(), "`wrap` may only be given once"));
                }
                wrap = Some(key);
            } else {
                return Err(Error::new(
                    key.span(),
                    "unknown argument, expected `range = MIN..MAX`, `range = MIN..=MAX` or `wrap`",
                ));
            }
        }
        Ok(BitsArgs { bits, range, wrap })
    }
}

// ---------------------------------------------------------------------------
// Field description
// ---------------------------------------------------------------------------

enum Kind {
    Bool,
    Unsigned,
    Signed,
    Float {
        min: Box<Expr>,
        max: Box<Expr>,
        inclusive: bool,
        wrap: bool,
        range_span: Span,
        range_text: String,
    },
    /// A type implementing `quantbits::BitEnum`, e.g. an enum.
    Custom,
}

struct FieldSpec {
    name: Ident,
    vis: Visibility,
    ty: Type,
    docs: Vec<Attribute>,
    kind: Kind,
    bits: u32,
    offset: usize,
}

/// Types the macro recognizes.
enum Prim {
    Bool,
    Unsigned(u32),
    Signed(u32),
    Float,
    /// Anything else that is a path, e.g. `Mode` or `crate::Mode`.
    Custom,
}

fn classify(ty: &Type) -> Option<Prim> {
    let Type::Path(tp) = ty else { return None };
    if tp.qself.is_some() {
        return None;
    }
    let Some(ident) = tp.path.get_ident() else {
        return Some(Prim::Custom);
    };
    Some(match ident.to_string().as_str() {
        "bool" => Prim::Bool,
        "u8" => Prim::Unsigned(8),
        "u16" => Prim::Unsigned(16),
        "u32" => Prim::Unsigned(32),
        "u64" => Prim::Unsigned(64),
        "i8" => Prim::Signed(8),
        "i16" => Prim::Signed(16),
        "i32" => Prim::Signed(32),
        "i64" => Prim::Signed(64),
        "f32" | "f64" => Prim::Float,
        // Unsupported primitive types, so that they get a clear error instead
        // of one about `BitEnum` not being implemented.
        "u128" | "i128" | "usize" | "isize" | "char" | "str" | "f16" | "f128" => return None,
        _ => Prim::Custom,
    })
}

fn compact(ts: impl ToTokens) -> String {
    ts.to_token_stream().to_string().replace(' ', "")
}

fn parse_field(field: &syn::Field, offset: usize) -> Result<FieldSpec> {
    let name = field.ident.clone().expect("named fields");
    let mut docs = Vec::new();
    let mut bits_attr: Option<(BitsArgs, Attribute)> = None;

    for attr in &field.attrs {
        if attr.path().is_ident("doc") {
            docs.push(attr.clone());
        } else if attr.path().is_ident("bits") {
            if bits_attr.is_some() {
                return Err(Error::new_spanned(attr, "`#[bits(...)]` may only be given once per field"));
            }
            bits_attr = Some((attr.parse_args::<BitsArgs>()?, attr.clone()));
        } else {
            return Err(Error::new_spanned(
                attr,
                "only `#[bits(...)]` and doc comments are supported on fields",
            ));
        }
    }

    let prim = classify(&field.ty).ok_or_else(|| {
        Error::new_spanned(
            &field.ty,
            "unsupported type, use bool, u8–u64, i8–i64, f32, f64 or a type with \
             `#[derive(quantbits::BitEnum)]`",
        )
    })?;

    let explicit_bits = match &bits_attr {
        Some((a, _)) => Some((a.bits.base10_parse::<u32>()?, a.bits.span())),
        None => None,
    };
    let range = bits_attr.as_ref().and_then(|(a, _)| a.range.clone());
    let wrap = bits_attr.as_ref().and_then(|(a, _)| a.wrap.clone());

    if !matches!(prim, Prim::Float) {
        if let Some(r) = &range {
            return Err(Error::new_spanned(r, "`range` only applies to f32/f64 fields"));
        }
        if let Some(w) = &wrap {
            return Err(Error::new(w.span(), "`wrap` only applies to f32/f64 fields"));
        }
    }

    let (kind, bits) = match prim {
        Prim::Bool => {
            if let Some((b, span)) = explicit_bits {
                if b != 1 {
                    return Err(Error::new(span, "a bool field is always 1 bit"));
                }
            }
            (Kind::Bool, 1)
        }
        Prim::Unsigned(w) | Prim::Signed(w) => {
            let bits = match explicit_bits {
                Some((b, span)) => {
                    if b == 0 || b > w {
                        return Err(Error::new(
                            span,
                            format!("number of bits must be 1..={w} for `{}`", compact(&field.ty)),
                        ));
                    }
                    b
                }
                None => w,
            };
            let kind = if matches!(prim, Prim::Unsigned(_)) { Kind::Unsigned } else { Kind::Signed };
            (kind, bits)
        }
        Prim::Float => {
            let Some((b, span)) = explicit_bits else {
                return Err(Error::new_spanned(
                    &field.ty,
                    "float fields require `#[bits(N, range = MIN..MAX)]`",
                ));
            };
            if b == 0 || b > 64 {
                return Err(Error::new(span, "number of bits must be 1..=64 for floats"));
            }
            let Some(r) = range else {
                return Err(Error::new_spanned(
                    &bits_attr.as_ref().unwrap().1,
                    "float fields require `range = MIN..MAX` or `range = MIN..=MAX`",
                ));
            };
            let range_span = r.span();
            let range_text = compact(&r);
            let (Some(min), Some(max)) = (r.start, r.end) else {
                return Err(Error::new(range_span, "the range must have both a start and an end"));
            };
            let inclusive = matches!(r.limits, RangeLimits::Closed(_));
            if let (Some(w), true) = (&wrap, inclusive) {
                return Err(Error::new(
                    w.span(),
                    "`wrap` requires a half-open range `MIN..MAX`, since MIN and MAX are \
                     the same point when values wrap around",
                ));
            }
            let wrap = wrap.is_some();
            (Kind::Float { min, max, inclusive, wrap, range_span, range_text }, b)
        }
        Prim::Custom => {
            let Some((b, span)) = explicit_bits else {
                return Err(Error::new_spanned(
                    &field.ty,
                    "custom types require `#[bits(N)]`, e.g. an enum with `#[derive(quantbits::BitEnum)]`",
                ));
            };
            if b == 0 || b > 64 {
                return Err(Error::new(span, "number of bits must be 1..=64"));
            }
            (Kind::Custom, b)
        }
    };

    Ok(FieldSpec { name, vis: field.vis.clone(), ty: field.ty.clone(), docs, kind, bits, offset })
}

/// The names of everything generated for a field, so that collisions get a
/// clear error instead of rustc's "duplicate definitions".
fn generated_names(s: &FieldSpec) -> Vec<String> {
    let base = s.name.unraw().to_string();
    let upper = base.to_uppercase();
    let mut names = vec![
        base.clone(),
        format!("set_{base}"),
        format!("with_{base}"),
        format!("{upper}_BITS"),
        format!("{upper}_OFFSET"),
    ];
    match s.kind {
        Kind::Unsigned | Kind::Signed => {
            names.push(format!("try_set_{base}"));
            names.push(format!("{upper}_MIN"));
            names.push(format!("{upper}_MAX"));
        }
        Kind::Float { .. } => {
            names.push(format!("set_{base}_stochastic"));
            names.push(format!("add_{base}_stochastic"));
            names.push(format!("sub_{base}_stochastic"));
            names.push(format!("{upper}_MIN"));
            names.push(format!("{upper}_MAX"));
            names.push(format!("{upper}_STEP"));
        }
        Kind::Bool | Kind::Custom => {}
    }
    names
}

// ---------------------------------------------------------------------------
// Code generation for `#[bitfields]`
// ---------------------------------------------------------------------------

fn expand(item: ItemStruct) -> Result<TokenStream2> {
    if !item.generics.params.is_empty() || item.generics.where_clause.is_some() {
        return Err(Error::new_spanned(&item.generics, "`#[bitfields]` does not support generic types"));
    }
    let Fields::Named(named) = &item.fields else {
        return Err(Error::new_spanned(&item.fields, "`#[bitfields]` requires a struct with named fields"));
    };

    // Parse all fields and report all errors at once.
    let mut specs = Vec::new();
    let mut errors: Option<Error> = None;
    let mut offset = 0usize;
    for f in &named.named {
        match parse_field(f, offset) {
            Ok(s) => {
                offset += s.bits as usize;
                specs.push(s);
            }
            Err(e) => combine(&mut errors, e),
        }
    }
    if let Some(e) = errors {
        return Err(e);
    }
    if specs.is_empty() {
        return Err(Error::new_spanned(&item.ident, "the struct needs at least one field"));
    }

    let mut taken: HashMap<String, String> =
        ["new", "from_bytes", "to_bytes", "as_bytes", "BYTES", "USED_BITS"]
            .iter()
            .map(|n| (n.to_string(), "a built-in method or constant".to_string()))
            .collect();
    for s in &specs {
        let base = s.name.unraw().to_string();
        for generated in generated_names(s) {
            if let Some(owner) = taken.get(&generated) {
                let msg = format!("field `{base}` generates `{generated}`, which already comes from {owner}");
                combine(&mut errors, Error::new(s.name.span(), msg));
            } else {
                taken.insert(generated, format!("field `{base}`"));
            }
        }
    }
    if let Some(e) = errors {
        return Err(e);
    }

    let used_bits = offset;
    let n_bytes = used_bits.div_ceil(8);

    // Attributes on the struct: Debug, Default, PartialOrd and Ord are taken out
    // of derive and generated here. Debug to show field values instead of raw
    // bytes, Default because std only implements Default for arrays of up to 32
    // elements, and PartialOrd/Ord to compare the fields in declaration order as
    // for an ordinary struct (the byte order says nothing about the field values).
    let mut want_debug = false;
    let mut want_default = false;
    let mut want_partial_ord = false;
    let mut want_ord = false;
    let mut attrs = Vec::new();
    for attr in &item.attrs {
        if attr.path().is_ident("derive") {
            let paths = attr.parse_args_with(Punctuated::<Path, Token![,]>::parse_terminated)?;
            let kept: Vec<&Path> = paths
                .iter()
                .filter(|p| match p.segments.last().map(|s| s.ident.to_string()).as_deref() {
                    Some("Debug") => {
                        want_debug = true;
                        false
                    }
                    Some("Default") => {
                        want_default = true;
                        false
                    }
                    Some("PartialOrd") => {
                        want_partial_ord = true;
                        false
                    }
                    Some("Ord") => {
                        want_ord = true;
                        false
                    }
                    _ => true,
                })
                .collect();
            if !kept.is_empty() {
                attrs.push(quote!(#[derive(#(#kept),*)]));
            }
        } else if attr.path().is_ident("repr") {
            return Err(Error::new_spanned(attr, "`#[bitfields]` sets `#[repr(transparent)]` itself"));
        } else {
            attrs.push(attr.to_token_stream());
        }
    }

    let name = &item.ident;
    let name_str = name.to_string();
    let vis = &item.vis;
    let p = quote!(::quantbits::__private);
    let rng_trait = quote!(::quantbits::RandomBits);
    let enum_trait = quote!(::quantbits::BitEnum);

    let mut accessors = Vec::new();
    let mut checks = Vec::new();
    for s in &specs {
        let FieldSpec { name: fname, vis: fvis, ty, docs, kind, bits, offset } = s;
        let (bits, offset) = (*bits, *offset);
        let get = fname;
        let set = format_ident!("set_{}", fname);
        let with = format_ident!("with_{}", fname);
        let fname_str = fname.unraw().to_string();
        let upper = fname_str.to_uppercase();
        let bits_const = format_ident!("{}_BITS", upper);
        let offset_const = format_ident!("{}_OFFSET", upper);
        let width = n_bits(bits);
        let location = format!("{width} ({})", bit_span(offset, bits));
        // A literal message, since `const fn` cannot format values.
        let overflow_msg = format!("quantbits: value for `{fname_str}` does not fit in {width}");

        // Trait methods cannot be called in `const fn`, so only custom types are non-const.
        let constness = if matches!(kind, Kind::Custom) { quote!() } else { quote!(const) };
        let mut extra = TokenStream2::new();

        let (info, read, write) = match kind {
            Kind::Bool => (
                format!("{location}."),
                quote!(#p::read_bits(&self.0, #offset, 1) != 0),
                quote!(#p::write_bits(&mut self.0, #offset, 1, value as u64);),
            ),
            Kind::Unsigned | Kind::Signed => {
                let signed = matches!(kind, Kind::Signed);
                let try_set = format_ident!("try_set_{}", fname);
                let min_const = format_ident!("{}_MIN", upper);
                let max_const = format_ident!("{}_MAX", upper);
                let (info, read, fits, raw, min, max) = if signed {
                    (
                        format!(
                            "{location}, values {}..={}.",
                            -(1i128 << (bits - 1)),
                            (1i128 << (bits - 1)) - 1
                        ),
                        quote!(#p::sign_extend(#p::read_bits(&self.0, #offset, #bits), #bits) as #ty),
                        quote!(#p::fits_signed(value as i64, #bits)),
                        quote!(value as i64 as u64),
                        quote!((-(1i128 << (#bits - 1))) as #ty),
                        quote!(((1i128 << (#bits - 1)) - 1) as #ty),
                    )
                } else {
                    (
                        format!("{location}, values 0..={}.", (1u128 << bits) - 1),
                        quote!(#p::read_bits(&self.0, #offset, #bits) as #ty),
                        quote!(#p::fits_unsigned(value as u64, #bits)),
                        quote!(value as u64),
                        quote!(0),
                        quote!(#p::mask(#bits) as #ty),
                    )
                };
                let try_doc = format!(
                    "Sets `{fname_str}` if the value fits in {width}, otherwise leaves the field unchanged."
                );
                let min_doc = format!("Smallest value of `{fname_str}`.");
                let max_doc = format!("Largest value of `{fname_str}`.");
                extra.extend(quote! {
                    #[doc = #min_doc]
                    #fvis const #min_const: #ty = #min;
                    #[doc = #max_doc]
                    #fvis const #max_const: #ty = #max;

                    #[doc = #try_doc]
                    #[inline]
                    #fvis const fn #try_set(&mut self, value: #ty)
                        -> ::core::result::Result<(), ::quantbits::OutOfRange>
                    {
                        if #fits {
                            #p::write_bits(&mut self.0, #offset, #bits, #raw);
                            ::core::result::Result::Ok(())
                        } else {
                            ::core::result::Result::Err(#p::out_of_range(#fname_str, #bits))
                        }
                    }
                });
                (
                    info,
                    read,
                    quote! {
                        ::core::debug_assert!(#fits, #overflow_msg);
                        #p::write_bits(&mut self.0, #offset, #bits, #raw);
                    },
                )
            }
            Kind::Float { min, max, inclusive, wrap, range_span, range_text } => {
                let msg =
                    format!("quantbits: field `{fname_str}` requires finite MIN < MAX in `{range_text}`");
                // Through the field's type first, so that e.g. `0.0..1e39` is rejected for f32.
                checks.push(quote_spanned! {*range_span=>
                    #[allow(clippy::unnecessary_cast, trivial_numeric_casts)]
                    const _: () = ::core::assert!(
                        #p::valid_range((#min) as #ty as f64, (#max) as #ty as f64, #bits, #inclusive),
                        #msg
                    );
                });
                let range = quote!((#min) as f64, (#max) as f64, #bits, #inclusive);
                let levels = if bits >= 64 { "2^64".to_string() } else { (1u128 << bits).to_string() };
                let ends = match (inclusive, wrap) {
                    (true, _) => "both endpoints included",
                    (false, false) => "MAX excluded",
                    (false, true) => "cyclic: values outside the range wrap around",
                };

                let set_st = format_ident!("set_{}_stochastic", fname);
                let add_st = format_ident!("add_{}_stochastic", fname);
                let sub_st = format_ident!("sub_{}_stochastic", fname);
                let min_const = format_ident!("{}_MIN", upper);
                let max_const = format_ident!("{}_MAX", upper);
                let step_const = format_ident!("{}_STEP", upper);
                let set_st_doc = format!(
                    "Sets `{fname_str}` with stochastic rounding: the value is rounded up with a \
                     probability equal to how far it lies towards the next level, so that the \
                     expected value is exactly `value` (within the range)."
                );
                let add_st_doc = format!(
                    "Adds `delta` to `{fname_str}` with stochastic rounding. The expected value of \
                     the result is exactly the old value plus `delta`, even when `delta` is much \
                     smaller than the step [`Self::{step_const}`], so many small increments add up \
                     correctly on average. Consumes one random number from `rng`."
                );
                let sub_st_doc = format!("Like [`Self::{add_st}`] but subtracts `delta`.");
                let min_doc = format!("Smallest value `{fname_str}` can hold.");
                let max_doc = format!("Largest value `{fname_str}` can hold.");
                let step_doc = format!("Distance between two adjacent levels of `{fname_str}`.");
                extra.extend(quote! {
                    #[doc = #min_doc]
                    #fvis const #min_const: #ty = #p::dequantize(0, #range) as #ty;
                    #[doc = #max_doc]
                    #fvis const #max_const: #ty = #p::dequantize(#p::mask(#bits), #range) as #ty;
                    #[doc = #step_doc]
                    #fvis const #step_const: #ty = ::quantbits::step(#range) as #ty;

                    #[doc = #set_st_doc]
                    #[inline]
                    #fvis fn #set_st(&mut self, value: #ty, rng: &mut impl #rng_trait) {
                        let x = #p::level(value as f64, #range);
                        let q = #p::stochastic_level(x, #bits, #wrap, #rng_trait::next_u64(rng));
                        #p::write_bits(&mut self.0, #offset, #bits, q);
                    }

                    #[doc = #add_st_doc]
                    #[inline]
                    #fvis fn #add_st(&mut self, delta: #ty, rng: &mut impl #rng_trait) {
                        let x = #p::read_bits(&self.0, #offset, #bits) as f64
                            + delta as f64 / ::quantbits::step(#range);
                        let q = #p::stochastic_level(x, #bits, #wrap, #rng_trait::next_u64(rng));
                        #p::write_bits(&mut self.0, #offset, #bits, q);
                    }

                    #[doc = #sub_st_doc]
                    #[inline]
                    #fvis fn #sub_st(&mut self, delta: #ty, rng: &mut impl #rng_trait) {
                        self.#add_st(-delta, rng);
                    }
                });
                (
                    format!("{location}, {levels} levels in `{range_text}`, {ends}."),
                    quote!(#p::dequantize(#p::read_bits(&self.0, #offset, #bits), #range) as #ty),
                    quote! {
                        let q = #p::quantize(value as f64, #range, #wrap);
                        #p::write_bits(&mut self.0, #offset, #bits, q);
                    },
                )
            }
            Kind::Custom => {
                let ty_str = compact(ty);
                let msg = format!(
                    "quantbits: field `{fname_str}` is too narrow for `{ty_str}`: \
                     `#[bits({bits})]` is less than `<{ty_str} as BitEnum>::BITS`"
                );
                checks.push(quote_spanned! {ty.span()=>
                    const _: () = ::core::assert!(<#ty as #enum_trait>::BITS <= #bits, #msg);
                });
                (
                    format!("{location}."),
                    quote!(<#ty as #enum_trait>::from_bits(#p::read_bits(&self.0, #offset, #bits))),
                    quote! {
                        let raw = <#ty as #enum_trait>::into_bits(value);
                        ::core::debug_assert!(#p::fits_unsigned(raw, #bits), #overflow_msg);
                        #p::write_bits(&mut self.0, #offset, #bits, raw);
                    },
                )
            }
        };

        let set_doc = format!("Sets `{fname_str}`.");
        let with_doc = format!("Like [`Self::{set}`] but returns `self`, for chained calls.");
        let bits_doc = format!("Number of bits in `{fname_str}`.");
        let offset_doc = format!("First bit of `{fname_str}`.");
        accessors.push(quote! {
            #[doc = #bits_doc]
            #fvis const #bits_const: u32 = #bits;
            #[doc = #offset_doc]
            #fvis const #offset_const: usize = #offset;

            #(#docs)*
            #[doc = ""]
            #[doc = #info]
            #[inline]
            #fvis #constness fn #get(&self) -> #ty { #read }

            #[doc = #set_doc]
            #[inline]
            #fvis #constness fn #set(&mut self, value: #ty) { #write }

            #[doc = #with_doc]
            #[inline]
            #[must_use]
            #fvis #constness fn #with(mut self, value: #ty) -> Self {
                self.#set(value);
                self
            }

            #extra
        });
    }

    let debug_impl = want_debug.then(|| {
        let names = specs.iter().map(|s| s.name.unraw().to_string());
        let gets = specs.iter().map(|s| &s.name);
        quote! {
            impl ::core::fmt::Debug for #name {
                fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                    f.debug_struct(#name_str)
                        #( .field(#names, &self.#gets()) )*
                        .finish()
                }
            }
        }
    });
    let default_impl = want_default.then(|| {
        quote! {
            impl ::core::default::Default for #name {
                #[inline]
                fn default() -> Self { Self::new() }
            }
        }
    });

    // Field-wise comparison in declaration order. Floats are compared by level,
    // which grows with the value, so the order is the same as for the decoded values.
    let cmp_fields = {
        let terms = specs.iter().map(|s| {
            let (offset, bits) = (s.offset, s.bits);
            let signed = matches!(s.kind, Kind::Signed);
            quote!(.then_with(|| #p::cmp_bits(&self.0, &other.0, #offset, #bits, #signed)))
        });
        quote!(::core::cmp::Ordering::Equal #(#terms)*)
    };
    let ord_impl = want_ord.then(|| {
        quote! {
            impl ::core::cmp::Ord for #name {
                #[inline]
                fn cmp(&self, other: &Self) -> ::core::cmp::Ordering { #cmp_fields }
            }
        }
    });
    let partial_ord_impl = want_partial_ord.then(|| {
        let body = if want_ord { quote!(::core::cmp::Ord::cmp(self, other)) } else { cmp_fields.clone() };
        quote! {
            impl ::core::cmp::PartialOrd for #name {
                #[inline]
                fn partial_cmp(&self, other: &Self) -> ::core::option::Option<::core::cmp::Ordering> {
                    ::core::option::Option::Some(#body)
                }
            }
        }
    });

    // The bits after the last field are always kept zero, so that PartialEq,
    // Hash and as_bytes depend only on the field values. The setters never
    // touch them, so it is enough to clear them in from_bytes.
    let pad_bits = used_bits % 8;
    let from_bytes = if pad_bits == 0 {
        quote!(pub const fn from_bytes(bytes: [u8; #n_bytes]) -> Self { Self(bytes) })
    } else {
        let last = n_bytes - 1;
        let keep = ((1u16 << pad_bits) - 1) as u8;
        quote! {
            pub const fn from_bytes(mut bytes: [u8; #n_bytes]) -> Self {
                bytes[#last] &= #keep;
                Self(bytes)
            }
        }
    };

    let byte_word = if n_bytes == 1 { "byte" } else { "bytes" };
    let struct_doc = format!("\n\nPacked into {n_bytes} {byte_word} ({} used).", n_bits(used_bits as u128));

    Ok(quote! {
        #(#attrs)*
        #[doc = #struct_doc]
        #[repr(transparent)]
        #vis struct #name([u8; #n_bytes]);

        impl #name {
            /// Number of bytes in the packed representation.
            pub const BYTES: usize = #n_bytes;
            /// Number of bits used by the fields.
            pub const USED_BITS: usize = #used_bits;

            /// All bits zero. Float fields then hold their MIN value.
            #[inline]
            pub const fn new() -> Self { Self([0u8; #n_bytes]) }

            /// Creates a value from raw bytes, e.g. read from the network or a file.
            /// The bit order is little-endian and platform independent.
            /// Unused bits after the last field are cleared.
            #[inline]
            #from_bytes

            /// A copy of the raw bytes.
            #[inline]
            pub const fn to_bytes(&self) -> [u8; #n_bytes] { self.0 }

            /// A reference to the raw bytes.
            #[inline]
            pub const fn as_bytes(&self) -> &[u8; #n_bytes] { &self.0 }
        }

        // `(#min) as f64` carries the user's spans, so rustc and clippy would
        // otherwise warn about e.g. `range = 0.0..1.0` in the user's code.
        #[allow(dead_code, trivial_numeric_casts, clippy::unnecessary_cast)]
        impl #name {
            #(#accessors)*
        }

        #(#checks)*
        #debug_impl
        #default_impl
        #partial_ord_impl
        #ord_impl
    })
}

// ---------------------------------------------------------------------------
// Code generation for `#[derive(BitEnum)]`
// ---------------------------------------------------------------------------

fn expand_bit_enum(item: DeriveInput) -> Result<TokenStream2> {
    let Data::Enum(data) = &item.data else {
        return Err(Error::new_spanned(&item.ident, "`BitEnum` can only be derived for enums"));
    };
    if !item.generics.params.is_empty() || item.generics.where_clause.is_some() {
        return Err(Error::new_spanned(&item.generics, "`BitEnum` does not support generic types"));
    }
    if data.variants.is_empty() {
        return Err(Error::new_spanned(&item.ident, "the enum needs at least one variant"));
    }

    let mut fallback: Option<&Ident> = None;
    let mut errors: Option<Error> = None;
    for v in &data.variants {
        if !matches!(v.fields, Fields::Unit) {
            combine(&mut errors, Error::new_spanned(&v.fields, "variants of a `BitEnum` cannot have fields"));
        }
        for attr in v.attrs.iter().filter(|a| a.path().is_ident("fallback")) {
            if let Err(e) = attr.meta.require_path_only() {
                combine(&mut errors, e);
            } else if fallback.is_some() {
                combine(&mut errors, Error::new_spanned(attr, "only one variant may be `#[fallback]`"));
            } else {
                fallback = Some(&v.ident);
            }
        }
    }
    if let Some(e) = errors {
        return Err(e);
    }

    let name = &item.ident;
    let name_str = name.to_string();
    let variants: Vec<&Ident> = data.variants.iter().map(|v| &v.ident).collect();
    let count = variants.len() as u64;
    let enum_trait = quote!(::quantbits::BitEnum);

    let non_negative = variants.iter().map(|v| {
        let msg = format!("quantbits: `{name_str}::{v}` has a negative discriminant");
        quote_spanned!(v.span()=> ::core::assert!((#name::#v as i128) >= 0, #msg);)
    });

    // Without a fallback every bit pattern must correspond to a variant. The
    // discriminants are distinct and all < 2^BITS, so counting them is enough.
    let (exhaustive_check, from_bits_tail, mask_input) = match fallback {
        Some(f) => (quote!(), quote!(Self::#f), quote!()),
        None => {
            let msg = format!(
                "quantbits: `{name_str}` does not cover every bit pattern; add variants \
                 or mark one variant with `#[fallback]`"
            );
            (
                quote_spanned! {name.span()=>
                    const _: () = ::core::assert!(
                        <#name as #enum_trait>::BITS < 64
                            && #count == 1u64 << <#name as #enum_trait>::BITS,
                        #msg
                    );
                },
                quote!(::core::unreachable!()),
                // A wider field may contain bits above BITS, e.g. via from_bytes.
                quote!(let bits = bits & ::quantbits::__private::mask(<Self as #enum_trait>::BITS);),
            )
        }
    };

    Ok(quote! {
        impl #enum_trait for #name {
            const BITS: u32 = {
                let mut max = 0u64;
                #( if (#name::#variants as u64) > max { max = #name::#variants as u64; } )*
                let bits = u64::BITS - max.leading_zeros();
                if bits == 0 { 1 } else { bits }
            };

            #[inline]
            fn into_bits(self) -> u64 {
                self as u64
            }

            #[inline]
            fn from_bits(bits: u64) -> Self {
                #mask_input
                #( if bits == Self::#variants as u64 { return Self::#variants; } )*
                #from_bits_tail
            }
        }

        const _: () = { #(#non_negative)* };
        #exhaustive_check
    })
}
