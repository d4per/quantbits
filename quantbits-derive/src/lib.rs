//! Procedural macros for `quantbits`. Use them via `quantbits::bitfields`,
//! `quantbits::codec` and `quantbits::BitEnum`, not directly.

#![forbid(unsafe_code)]

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
    let args = parse_macro_input!(args as BitfieldsArgs);
    let item = parse_macro_input!(input as ItemStruct);
    match expand(args, item) {
        Ok(ts) => ts.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

/// Turns `type Name = T;` into a [`Codec`](https://docs.rs/quantbits/latest/quantbits/trait.Codec.html)
/// that stores `T` like a field with `#[bits(...)]` and the same arguments.
#[proc_macro_attribute]
pub fn codec(args: TokenStream, input: TokenStream) -> TokenStream {
    let args = TokenStream2::from(args);
    if args.is_empty() {
        let msg = "`#[codec]` needs the number of bits, e.g. `#[codec(5)]` or \
                   `#[codec(7, range = -1.0..=1.0)]`";
        return Error::new(Span::call_site(), msg).to_compile_error().into();
    }
    let parsed = match syn::parse2::<BitsArgs>(args.clone()) {
        Ok(a) => a,
        Err(e) => return e.to_compile_error().into(),
    };
    let item = parse_macro_input!(input as syn::Item);
    match expand_codec(parsed, args, item) {
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
                match strip_expr_groups(input.parse::<Expr>()?) {
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

/// The arguments of `#[bitfields(...)]`.
struct BitfieldsArgs {
    columns: Option<Ident>,
}

impl Parse for BitfieldsArgs {
    fn parse(input: ParseStream) -> Result<Self> {
        let mut columns = None;
        while !input.is_empty() {
            let key: Ident = input.parse()?;
            if key != "columns" {
                return Err(Error::new(key.span(), "unknown argument, expected `columns = Name`"));
            }
            if columns.is_some() {
                return Err(Error::new(key.span(), "`columns` may only be given once"));
            }
            input.parse::<Token![=]>()?;
            columns = Some(input.parse::<Ident>()?);
            if input.is_empty() {
                break;
            }
            input.parse::<Token![,]>()?;
        }
        Ok(BitfieldsArgs { columns })
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

/// `ty` without parentheses and the invisible groups that `macro_rules!`
/// puts around a `$t:ty`.
fn strip_type_groups(mut ty: &Type) -> &Type {
    loop {
        match ty {
            Type::Group(g) => ty = &g.elem,
            Type::Paren(p) => ty = &p.elem,
            _ => return ty,
        }
    }
}

/// Like [`strip_type_groups`], for a `$e:expr`.
fn strip_expr_groups(mut expr: Expr) -> Expr {
    loop {
        match expr {
            Expr::Group(g) => expr = *g.expr,
            Expr::Paren(p) => expr = *p.expr,
            _ => return expr,
        }
    }
}

fn classify(ty: &Type) -> Option<Prim> {
    let Type::Path(tp) = strip_type_groups(ty) else { return None };
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

/// Where a bit specification comes from, for the wording of error messages.
#[derive(Clone, Copy)]
enum Ctx {
    Field,
    Codec,
}

impl Ctx {
    fn noun(self) -> &'static str {
        match self {
            Ctx::Field => "field",
            Ctx::Codec => "codec",
        }
    }
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

    let args = bits_attr.map(|(a, attr)| (a, attr.to_token_stream()));
    let (kind, bits) = parse_spec(&field.ty, args, Ctx::Field)?;
    Ok(FieldSpec { name, vis: field.vis.clone(), ty: field.ty.clone(), docs, kind, bits, offset })
}

/// The kind and width of a value of type `ty` with the given `#[bits(...)]`
/// arguments and the tokens they come from (for error spans).
fn parse_spec(ty: &Type, args: Option<(BitsArgs, TokenStream2)>, ctx: Ctx) -> Result<(Kind, u32)> {
    let noun = ctx.noun();
    let prim = classify(ty).ok_or_else(|| {
        Error::new_spanned(
            ty,
            "unsupported type, use bool, u8–u64, i8–i64, f32, f64 or a type with \
             `#[derive(quantbits::BitEnum)]`",
        )
    })?;

    let explicit_bits = match &args {
        Some((a, _)) => Some((a.bits.base10_parse::<u32>()?, a.bits.span())),
        None => None,
    };
    let range = args.as_ref().and_then(|(a, _)| a.range.clone());
    let wrap = args.as_ref().and_then(|(a, _)| a.wrap.clone());

    if !matches!(prim, Prim::Float) {
        if let Some(r) = &range {
            return Err(Error::new_spanned(r, format!("`range` only applies to f32/f64 {noun}s")));
        }
        if let Some(w) = &wrap {
            return Err(Error::new(w.span(), format!("`wrap` only applies to f32/f64 {noun}s")));
        }
    }

    Ok(match prim {
        Prim::Bool => {
            if let Some((b, span)) = explicit_bits {
                if b != 1 {
                    return Err(Error::new(span, format!("a bool {noun} is always 1 bit")));
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
                            format!("number of bits must be 1..={w} for `{}`", compact(ty)),
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
                return Err(Error::new_spanned(ty, "float fields require `#[bits(N, range = MIN..MAX)]`"));
            };
            if b == 0 || b > 64 {
                return Err(Error::new(span, "number of bits must be 1..=64 for floats"));
            }
            let Some(r) = range else {
                return Err(Error::new_spanned(
                    &args.as_ref().unwrap().1,
                    format!("float {noun}s require `range = MIN..MAX` or `range = MIN..=MAX`"),
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
                    ty,
                    "custom types require `#[bits(N)]`, e.g. an enum with `#[derive(quantbits::BitEnum)]`",
                ));
            };
            if b == 0 || b > 64 {
                return Err(Error::new(span, "number of bits must be 1..=64"));
            }
            (Kind::Custom, b)
        }
    })
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
// Conversion between values and raw bits, shared by all generated code
// ---------------------------------------------------------------------------

/// The trailing arguments of the float helpers in `__private`.
fn float_range(min: &Expr, max: &Expr, bits: u32, inclusive: bool) -> TokenStream2 {
    quote!((#min) as f64, (#max) as f64, #bits, #inclusive)
}

/// Raw bits (an expression of type `u64` that fits in `bits`) -> value of type `ty`.
fn decode_expr(kind: &Kind, ty: &Type, bits: u32, raw: TokenStream2) -> TokenStream2 {
    let p = quote!(::quantbits::__private);
    match kind {
        Kind::Bool => quote!((#raw) != 0),
        Kind::Unsigned => quote!((#raw) as #ty),
        Kind::Signed => quote!(#p::sign_extend(#raw, #bits) as #ty),
        Kind::Float { min, max, inclusive, .. } => {
            let range = float_range(min, max, bits, *inclusive);
            quote!(#p::dequantize(#raw, #range) as #ty)
        }
        Kind::Custom => quote!(<#ty as ::quantbits::BitEnum>::from_bits(#raw)),
    }
}

/// Value -> raw bits without any check. Only the lowest `bits` bits are kept
/// when writing. Floats are rounded and clamped (or wrapped).
fn raw_expr(kind: &Kind, ty: &Type, bits: u32, value: &Ident) -> TokenStream2 {
    let p = quote!(::quantbits::__private);
    match kind {
        Kind::Bool | Kind::Unsigned => quote!(#value as u64),
        Kind::Signed => quote!(#value as i64 as u64),
        Kind::Float { min, max, inclusive, wrap, .. } => {
            let range = float_range(min, max, bits, *inclusive);
            quote!(#p::quantize(#value as f64, #range, #wrap))
        }
        Kind::Custom => quote!(<#ty as ::quantbits::BitEnum>::into_bits(#value)),
    }
}

/// Whether `raw` (from [`raw_expr`]) fits in the field, for integers and enums.
fn fits_expr(kind: &Kind, bits: u32, raw: &Ident) -> Option<TokenStream2> {
    let p = quote!(::quantbits::__private);
    match kind {
        Kind::Unsigned | Kind::Custom => Some(quote!(#p::fits_unsigned(#raw, #bits))),
        Kind::Signed => Some(quote!(#p::fits_signed(#raw as i64, #bits))),
        Kind::Bool | Kind::Float { .. } => None,
    }
}

/// Value -> raw bits, with a `debug_assert!` for integers and enums that do not fit.
fn encode_expr(kind: &Kind, ty: &Type, bits: u32, value: &Ident, overflow_msg: &str) -> TokenStream2 {
    let raw_ident = format_ident!("raw");
    let raw = raw_expr(kind, ty, bits, value);
    match fits_expr(kind, bits, &raw_ident) {
        Some(fits) => quote!({
            let #raw_ident: u64 = #raw;
            ::core::debug_assert!(#fits, #overflow_msg);
            #raw_ident
        }),
        None => raw,
    }
}

/// What a value can hold, for the docs, e.g. "values 0..=31".
fn value_info(kind: &Kind, bits: u32) -> Option<String> {
    match kind {
        Kind::Unsigned => Some(format!("values 0..={}", (1u128 << bits) - 1)),
        Kind::Signed => Some(format!("values {}..={}", -(1i128 << (bits - 1)), (1i128 << (bits - 1)) - 1)),
        Kind::Float { inclusive, wrap, range_text, .. } => {
            let levels = if bits >= 64 { "2^64".to_string() } else { (1u128 << bits).to_string() };
            let ends = match (inclusive, wrap) {
                (true, _) => "both endpoints included",
                (false, false) => "MAX excluded",
                (false, true) => "cyclic: values outside the range wrap around",
            };
            Some(format!("{levels} levels in `{range_text}`, {ends}"))
        }
        Kind::Bool | Kind::Custom => None,
    }
}

/// The smallest and largest value of an integer or float, as constant
/// expressions of type `ty`.
fn limits_expr(kind: &Kind, ty: &Type, bits: u32) -> Option<(TokenStream2, TokenStream2)> {
    let p = quote!(::quantbits::__private);
    match kind {
        Kind::Unsigned => Some((quote!(0), quote!(#p::mask(#bits) as #ty))),
        Kind::Signed => {
            Some((quote!((-(1i128 << (#bits - 1))) as #ty), quote!(((1i128 << (#bits - 1)) - 1) as #ty)))
        }
        Kind::Float { min, max, inclusive, .. } => {
            let range = float_range(min, max, bits, *inclusive);
            Some((
                quote!(#p::dequantize(0, #range) as #ty),
                quote!(#p::dequantize(#p::mask(#bits), #range) as #ty),
            ))
        }
        Kind::Bool | Kind::Custom => None,
    }
}

/// Compile-time checks: a valid float range, or an enum that fits. `what`
/// names the field or codec in the message, e.g. "field `x`".
fn spec_check(kind: &Kind, ty: &Type, bits: u32, what: &str) -> Option<TokenStream2> {
    let p = quote!(::quantbits::__private);
    match kind {
        Kind::Float { min, max, inclusive, range_span, range_text, .. } => {
            let msg = format!("quantbits: {what} requires finite MIN < MAX in `{range_text}`");
            // Through the field's type first, so that e.g. `0.0..1e39` is rejected for f32.
            Some(quote_spanned! {*range_span=>
                #[allow(clippy::unnecessary_cast, trivial_numeric_casts)]
                const _: () = ::core::assert!(
                    #p::valid_range((#min) as #ty as f64, (#max) as #ty as f64, #bits, #inclusive),
                    #msg
                );
            })
        }
        Kind::Custom => {
            let ty_str = compact(ty);
            let msg = format!(
                "quantbits: {what} is too narrow for `{ty_str}`: \
                 `#[bits({bits})]` is less than `<{ty_str} as BitEnum>::BITS`"
            );
            Some(quote_spanned! {ty.span()=>
                const _: () = ::core::assert!(<#ty as ::quantbits::BitEnum>::BITS <= #bits, #msg);
            })
        }
        Kind::Bool | Kind::Unsigned | Kind::Signed => None,
    }
}

/// Float value -> level with stochastic rounding. `random` is an expression
/// giving 64 random bits.
fn stochastic_set_expr(kind: &Kind, bits: u32, value: &Ident, random: TokenStream2) -> TokenStream2 {
    let p = quote!(::quantbits::__private);
    let Kind::Float { min, max, inclusive, wrap, .. } = kind else { unreachable!("float fields only") };
    let range = float_range(min, max, bits, *inclusive);
    quote!({
        let x = #p::level(#value as f64, #range);
        #p::stochastic_level(x, #bits, #wrap, #random)
    })
}

/// Level `raw` plus the float `delta`, with stochastic rounding.
fn stochastic_add_expr(
    kind: &Kind,
    bits: u32,
    raw: TokenStream2,
    delta: &Ident,
    random: TokenStream2,
) -> TokenStream2 {
    let p = quote!(::quantbits::__private);
    let Kind::Float { min, max, inclusive, wrap, .. } = kind else { unreachable!("float fields only") };
    let range = float_range(min, max, bits, *inclusive);
    quote!({
        let x = (#raw) as f64 + #delta as f64 / ::quantbits::step(#range);
        #p::stochastic_level(x, #bits, #wrap, #random)
    })
}

// ---------------------------------------------------------------------------
// Code generation for `#[bitfields]`
// ---------------------------------------------------------------------------

fn expand(args: BitfieldsArgs, item: ItemStruct) -> Result<TokenStream2> {
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

        let read = decode_expr(kind, ty, bits, quote!(#p::read_bits(&self.0, #offset, #bits)));
        let value = format_ident!("value");
        let encoded = encode_expr(kind, ty, bits, &value, &overflow_msg);
        let write = quote!(#p::write_bits(&mut self.0, #offset, #bits, #encoded););
        checks.extend(spec_check(kind, ty, bits, &format!("field `{fname_str}`")));

        let info = match value_info(kind, bits) {
            Some(values) => format!("{location}, {values}."),
            None => format!("{location}."),
        };

        match kind {
            Kind::Bool | Kind::Custom => {}
            Kind::Unsigned | Kind::Signed => {
                let try_set = format_ident!("try_set_{}", fname);
                let min_const = format_ident!("{}_MIN", upper);
                let max_const = format_ident!("{}_MAX", upper);
                let (min, max) = limits_expr(kind, ty, bits).expect("integer");
                let raw_ident = format_ident!("raw");
                let raw = raw_expr(kind, ty, bits, &value);
                let fits = fits_expr(kind, bits, &raw_ident);
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
                        let #raw_ident: u64 = #raw;
                        if #fits {
                            #p::write_bits(&mut self.0, #offset, #bits, #raw_ident);
                            ::core::result::Result::Ok(())
                        } else {
                            ::core::result::Result::Err(#p::out_of_range(#fname_str, #bits))
                        }
                    }
                });
            }
            Kind::Float { min, max, inclusive, .. } => {
                let range = float_range(min, max, bits, *inclusive);
                let (min_value, max_value) = limits_expr(kind, ty, bits).expect("float");

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
                let set_q = stochastic_set_expr(kind, bits, &value, quote!(#rng_trait::next_u64(rng)));
                let delta = format_ident!("delta");
                let add_q = stochastic_add_expr(
                    kind,
                    bits,
                    quote!(#p::read_bits(&self.0, #offset, #bits)),
                    &delta,
                    quote!(#rng_trait::next_u64(rng)),
                );
                extra.extend(quote! {
                    #[doc = #min_doc]
                    #fvis const #min_const: #ty = #min_value;
                    #[doc = #max_doc]
                    #fvis const #max_const: #ty = #max_value;
                    #[doc = #step_doc]
                    #fvis const #step_const: #ty = ::quantbits::step(#range) as #ty;

                    #[doc = #set_st_doc]
                    #[inline]
                    #fvis fn #set_st(&mut self, value: #ty, rng: &mut impl #rng_trait) {
                        let q = #set_q;
                        #p::write_bits(&mut self.0, #offset, #bits, q);
                    }

                    #[doc = #add_st_doc]
                    #[inline]
                    #fvis fn #add_st(&mut self, delta: #ty, rng: &mut impl #rng_trait) {
                        let q = #add_q;
                        #p::write_bits(&mut self.0, #offset, #bits, q);
                    }

                    #[doc = #sub_st_doc]
                    #[inline]
                    #fvis fn #sub_st(&mut self, delta: #ty, rng: &mut impl #rng_trait) {
                        self.#add_st(-delta, rng);
                    }
                });
            }
        }

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

    // A struct of at most 64 bits can itself be a value in a packed container.
    let codec_impl = (used_bits <= 64).then(|| {
        let bits = used_bits as u32;
        quote! {
            impl ::quantbits::Codec for #name {
                type Value = Self;
                const BITS: u32 = #bits;

                #[inline]
                fn encode(value: Self) -> u64 {
                    let mut raw = [0u8; 8];
                    raw[..#n_bytes].copy_from_slice(&value.0);
                    u64::from_le_bytes(raw)
                }

                #[inline]
                fn decode(raw: u64) -> Self {
                    let mut bytes = [0u8; #n_bytes];
                    bytes.copy_from_slice(&raw.to_le_bytes()[..#n_bytes]);
                    Self::from_bytes(bytes)
                }
            }
        }
    });
    let columns_impl = match &args.columns {
        Some(columns) => Some(expand_columns(columns, name, vis, &specs, want_debug)?),
        None => None,
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
        #codec_impl
        #columns_impl
    })
}

// ---------------------------------------------------------------------------
// Code generation for `#[bitfields(columns = Name)]`
// ---------------------------------------------------------------------------

/// A struct of arrays: one `PackedVec` of raw bits per field.
fn expand_columns(
    columns: &Ident,
    name: &Ident,
    vis: &Visibility,
    specs: &[FieldSpec],
    want_debug: bool,
) -> Result<TokenStream2> {
    let p = quote!(::quantbits::__private);
    let rng_trait = quote!(::quantbits::RandomBits);
    let columns_str = columns.to_string();
    let name_str = name.to_string();

    let builtins = [
        "new",
        "with_capacity",
        "len",
        "is_empty",
        "reserve",
        "clear",
        "truncate",
        "push",
        "pop",
        "get",
        "set",
        "iter",
        "__row",
    ];
    let mut taken: HashMap<String, String> =
        builtins.iter().map(|n| (n.to_string(), "a built-in method".to_string())).collect();
    let mut errors: Option<Error> = None;
    for s in specs {
        let base = s.name.unraw().to_string();
        let mut names =
            vec![base.clone(), format!("set_{base}"), format!("{base}_iter"), format!("{base}_column")];
        match s.kind {
            Kind::Unsigned | Kind::Signed => names.push(format!("try_set_{base}")),
            Kind::Float { .. } => {
                names.push(format!("set_{base}_stochastic"));
                names.push(format!("add_{base}_stochastic"));
                names.push(format!("sub_{base}_stochastic"));
            }
            Kind::Bool | Kind::Custom => {}
        }
        for generated in names {
            if let Some(owner) = taken.get(&generated) {
                let msg = format!(
                    "field `{base}` generates `{columns_str}::{generated}`, which already comes from {owner}"
                );
                combine(&mut errors, Error::new(s.name.span(), msg));
            } else {
                taken.insert(generated, format!("field `{base}`"));
            }
        }
    }
    if let Some(e) = errors {
        return Err(e);
    }

    let names: Vec<&Ident> = specs.iter().map(|s| &s.name).collect();
    let first = names[0];
    let raw_ident = format_ident!("raw");
    let value = format_ident!("value");
    let delta = format_ident!("delta");
    let mut fields = Vec::new();
    let mut pushes = Vec::new();
    let mut sets = Vec::new();
    let mut reads = Vec::new();
    let mut methods = Vec::new();
    for s in specs {
        let FieldSpec { name: f, vis: fvis, ty, kind, bits, offset, .. } = s;
        let (bits, offset) = (*bits, *offset);
        let column_ty = quote!(::quantbits::Int<u64, #bits>);
        fields.push(quote!(#f: ::quantbits::PackedVec<#column_ty>));
        pushes.push(quote!(self.#f.push_raw(#p::read_bits(&value.0, #offset, #bits));));
        sets.push(quote!(self.#f.set_raw(index, #p::read_bits(&value.0, #offset, #bits));));
        reads.push(quote!(#p::write_bits(&mut row.0, #offset, #bits, self.#f.get_raw(index).unwrap());));

        let f_str = f.unraw().to_string();
        let set = format_ident!("set_{}", f);
        let iter = format_ident!("{}_iter", f);
        let column = format_ident!("{}_column", f);
        let overflow_msg = format!("quantbits: value for `{f_str}` does not fit in {}", n_bits(bits));
        let decode = decode_expr(kind, ty, bits, quote!(#raw_ident));
        let encoded = encode_expr(kind, ty, bits, &value, &overflow_msg);
        let get_doc = format!("`{f_str}` of the row at `index`. Panics if `index` is out of bounds.");
        let set_doc = format!("Sets `{f_str}` of the row at `index`. Panics if `index` is out of bounds.");
        let iter_doc = format!("An iterator over `{f_str}` of every row. Only reads this column.");
        let column_doc = format!(
            "The raw bits of `{f_str}`, packed with {} per row, e.g. for saving the column.",
            n_bits(bits)
        );

        let mut extra = TokenStream2::new();
        match kind {
            Kind::Bool | Kind::Custom => {}
            Kind::Unsigned | Kind::Signed => {
                let try_set = format_ident!("try_set_{}", f);
                let raw = raw_expr(kind, ty, bits, &value);
                let fits = fits_expr(kind, bits, &raw_ident);
                let try_doc = format!(
                    "Sets `{f_str}` of the row at `index` if the value fits in {}, otherwise leaves \
                     it unchanged. Panics if `index` is out of bounds.",
                    n_bits(bits)
                );
                extra.extend(quote! {
                    #[doc = #try_doc]
                    #[inline]
                    #[track_caller]
                    #fvis fn #try_set(&mut self, index: usize, value: #ty)
                        -> ::core::result::Result<(), ::quantbits::OutOfRange>
                    {
                        if index >= self.len() {
                            #p::index_out_of_bounds(index, self.len());
                        }
                        let #raw_ident: u64 = #raw;
                        if #fits {
                            self.#f.set_raw(index, #raw_ident);
                            ::core::result::Result::Ok(())
                        } else {
                            ::core::result::Result::Err(#p::out_of_range(#f_str, #bits))
                        }
                    }
                });
            }
            Kind::Float { .. } => {
                let set_st = format_ident!("set_{}_stochastic", f);
                let add_st = format_ident!("add_{}_stochastic", f);
                let sub_st = format_ident!("sub_{}_stochastic", f);
                let random = quote!(#rng_trait::next_u64(rng));
                let set_q = stochastic_set_expr(kind, bits, &value, random.clone());
                let add_q = stochastic_add_expr(kind, bits, quote!(#raw_ident), &delta, random);
                let set_st_doc = format!(
                    "Sets `{f_str}` of the row at `index` with stochastic rounding. Panics if \
                     `index` is out of bounds."
                );
                let add_st_doc = format!(
                    "Adds `delta` to `{f_str}` of the row at `index` with stochastic rounding, so \
                     that many small increments add up correctly on average. Panics if `index` is \
                     out of bounds."
                );
                let sub_st_doc = format!("Like [`Self::{add_st}`] but subtracts `delta`.");
                extra.extend(quote! {
                    #[doc = #set_st_doc]
                    #[inline]
                    #[track_caller]
                    #fvis fn #set_st(&mut self, index: usize, value: #ty, rng: &mut impl #rng_trait) {
                        if index >= self.len() {
                            #p::index_out_of_bounds(index, self.len());
                        }
                        let q = #set_q;
                        self.#f.set_raw(index, q);
                    }

                    #[doc = #add_st_doc]
                    #[inline]
                    #[track_caller]
                    #fvis fn #add_st(&mut self, index: usize, delta: #ty, rng: &mut impl #rng_trait) {
                        let #raw_ident = match self.#f.get_raw(index) {
                            ::core::option::Option::Some(raw) => raw,
                            ::core::option::Option::None => #p::index_out_of_bounds(index, self.len()),
                        };
                        let q = #add_q;
                        self.#f.set_raw(index, q);
                    }

                    #[doc = #sub_st_doc]
                    #[inline]
                    #[track_caller]
                    #fvis fn #sub_st(&mut self, index: usize, delta: #ty, rng: &mut impl #rng_trait) {
                        self.#add_st(index, -delta, rng);
                    }
                });
            }
        }

        methods.push(quote! {
            #[doc = #get_doc]
            #[inline]
            #[track_caller]
            #fvis fn #f(&self, index: usize) -> #ty {
                match self.#f.get_raw(index) {
                    ::core::option::Option::Some(#raw_ident) => #decode,
                    ::core::option::Option::None => #p::index_out_of_bounds(index, self.len()),
                }
            }

            #[doc = #set_doc]
            #[inline]
            #[track_caller]
            #fvis fn #set(&mut self, index: usize, value: #ty) {
                self.#f.set_raw(index, #encoded);
            }

            #[doc = #iter_doc]
            #[inline]
            #fvis fn #iter(
                &self,
            ) -> impl ::core::iter::ExactSizeIterator<Item = #ty> + ::core::iter::DoubleEndedIterator + '_ {
                self.#f.iter().map(|#raw_ident| #decode)
            }

            #[doc = #column_doc]
            #[inline]
            #fvis fn #column(&self) -> ::quantbits::PackedSlice<'_, #column_ty> {
                self.#f.as_slice()
            }

            #extra
        });
    }

    let doc = format!(
        "[`{name_str}`] stored as one packed column per field (a struct of arrays), generated by \
         `#[bitfields(columns = {columns_str})]`.\n\nReading or iterating a single field, e.g. with \
         [`{first_iter}`](Self::{first_iter}), only touches that field's column.",
        first_iter = format_ident!("{}_iter", first),
    );
    let debug_impl = want_debug.then(|| {
        quote! {
            impl ::core::fmt::Debug for #columns {
                fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                    f.debug_list().entries(self.iter()).finish()
                }
            }
        }
    });

    let items = quote! {
        #[doc = #doc]
        #[derive(Clone, Default, PartialEq, Eq, Hash)]
        #vis struct #columns {
            #(#fields,)*
        }

        #[allow(dead_code, trivial_numeric_casts, clippy::unnecessary_cast)]
        impl #columns {
            /// An empty table. Does not allocate.
            #[inline]
            pub const fn new() -> Self {
                Self { #(#names: ::quantbits::PackedVec::new(),)* }
            }

            /// An empty table with room for at least `capacity` rows.
            pub fn with_capacity(capacity: usize) -> Self {
                Self { #(#names: ::quantbits::PackedVec::with_capacity(capacity),)* }
            }

            /// Number of rows.
            #[inline]
            pub fn len(&self) -> usize {
                self.#first.len()
            }

            /// Whether there are no rows.
            #[inline]
            pub fn is_empty(&self) -> bool {
                self.#first.is_empty()
            }

            /// Reserves room for at least `additional` more rows.
            pub fn reserve(&mut self, additional: usize) {
                #(self.#names.reserve(additional);)*
            }

            /// Removes all rows.
            pub fn clear(&mut self) {
                #(self.#names.clear();)*
            }

            /// Shortens the table to `len` rows. Has no effect if it is not longer.
            pub fn truncate(&mut self, len: usize) {
                #(self.#names.truncate(len);)*
            }

            /// Appends a row.
            pub fn push(&mut self, value: #name) {
                #(#pushes)*
            }

            /// Removes and returns the last row, or `None` if empty.
            pub fn pop(&mut self) -> ::core::option::Option<#name> {
                let last = self.len().checked_sub(1)?;
                let row = self.__row(last);
                self.truncate(last);
                ::core::option::Option::Some(row)
            }

            /// The row at `index`, or `None` if out of bounds.
            #[inline]
            pub fn get(&self, index: usize) -> ::core::option::Option<#name> {
                if index < self.len() {
                    ::core::option::Option::Some(self.__row(index))
                } else {
                    ::core::option::Option::None
                }
            }

            /// Sets the row at `index`. Panics if `index` is out of bounds.
            #[track_caller]
            pub fn set(&mut self, index: usize, value: #name) {
                if index >= self.len() {
                    #p::index_out_of_bounds(index, self.len());
                }
                #(#sets)*
            }

            /// An iterator over the rows.
            #[inline]
            pub fn iter(&self) -> ::quantbits::Rows<'_, Self> {
                #p::rows(self)
            }

            #[inline]
            fn __row(&self, index: usize) -> #name {
                let mut row = #name::new();
                #(#reads)*
                row
            }

            #(#methods)*
        }

        impl #p::Table for #columns {
            type Row = #name;

            #[inline]
            fn __len(&self) -> usize {
                self.len()
            }

            #[inline]
            fn __row(&self, index: usize) -> #name {
                #columns::__row(self, index)
            }
        }

        impl<'a> ::core::iter::IntoIterator for &'a #columns {
            type Item = #name;
            type IntoIter = ::quantbits::Rows<'a, #columns>;

            #[inline]
            fn into_iter(self) -> ::quantbits::Rows<'a, #columns> {
                self.iter()
            }
        }

        impl ::core::iter::Extend<#name> for #columns {
            fn extend<I: ::core::iter::IntoIterator<Item = #name>>(&mut self, iter: I) {
                let iter = iter.into_iter();
                self.reserve(iter.size_hint().0);
                for value in iter {
                    self.push(value);
                }
            }
        }

        impl ::core::iter::FromIterator<#name> for #columns {
            fn from_iter<I: ::core::iter::IntoIterator<Item = #name>>(iter: I) -> Self {
                let mut columns = Self::new();
                columns.extend(iter);
                columns
            }
        }

        #debug_impl
    };
    // Expands to `items` only with the `alloc` feature, and otherwise to an
    // error pointing at `Name`.
    Ok(quote_spanned!(columns.span()=> ::quantbits::__require_alloc! { #items }))
}

// ---------------------------------------------------------------------------
// Code generation for `#[codec]`
// ---------------------------------------------------------------------------

fn expand_codec(args: BitsArgs, args_tokens: TokenStream2, item: syn::Item) -> Result<TokenStream2> {
    let syn::Item::Type(item) = item else {
        return Err(Error::new_spanned(
            item,
            "`#[codec]` must be put on a type alias, e.g. `pub type Level = f32;`",
        ));
    };
    if !item.generics.params.is_empty() || item.generics.where_clause.is_some() {
        return Err(Error::new_spanned(&item.generics, "`#[codec]` does not support generic types"));
    }
    let ty = &*item.ty;
    let (kind, bits) = parse_spec(ty, Some((args, args_tokens)), Ctx::Codec)?;
    let name = &item.ident;
    let vis = &item.vis;
    let attrs = &item.attrs;
    let name_str = name.to_string();
    let p = quote!(::quantbits::__private);

    let raw_ident = format_ident!("raw");
    let value = format_ident!("value");
    let delta = format_ident!("delta");
    let overflow_msg = format!("quantbits: value does not fit in `{name_str}`");
    let decode = decode_expr(&kind, ty, bits, quote!(#raw_ident));
    let encode = encode_expr(&kind, ty, bits, &value, &overflow_msg);
    let try_encode = fits_expr(&kind, bits, &raw_ident).map(|fits| {
        let raw = raw_expr(&kind, ty, bits, &value);
        quote! {
            #[inline]
            fn try_encode(value: #ty) -> ::core::result::Result<u64, ::quantbits::OutOfRange> {
                let #raw_ident: u64 = #raw;
                if #fits {
                    ::core::result::Result::Ok(#raw_ident)
                } else {
                    ::core::result::Result::Err(#p::out_of_range(#name_str, #bits))
                }
            }
        }
    });
    let check = spec_check(&kind, ty, bits, &format!("`{name_str}`"));

    let width = n_bits(bits);
    let ty_str = compact(ty);
    let info = match value_info(&kind, bits) {
        Some(values) => format!("Stores `{ty_str}` in {width}, {values}."),
        None => format!("Stores `{ty_str}` in {width}."),
    };
    let limits = limits_expr(&kind, ty, bits).map(|(min, max)| {
        quote! {
            /// Smallest value that can be stored.
            pub const MIN: #ty = #min;
            /// Largest value that can be stored.
            pub const MAX: #ty = #max;
        }
    });
    let (step, stochastic_impl) = match &kind {
        Kind::Float { min, max, inclusive, .. } => {
            let range = float_range(min, max, bits, *inclusive);
            let set_q = stochastic_set_expr(&kind, bits, &value, quote!(random));
            let add_q = stochastic_add_expr(&kind, bits, quote!(#raw_ident), &delta, quote!(random));
            (
                Some(quote! {
                    /// Distance between two adjacent levels.
                    pub const STEP: #ty = ::quantbits::step(#range) as #ty;
                }),
                Some(quote! {
                    #[allow(trivial_numeric_casts, clippy::unnecessary_cast)]
                    impl ::quantbits::StochasticCodec for #name {
                        #[inline]
                        fn encode_stochastic(value: #ty, random: u64) -> u64 {
                            #set_q
                        }

                        #[inline]
                        fn add_stochastic(raw: u64, delta: #ty, random: u64) -> u64 {
                            #add_q
                        }
                    }
                }),
            )
        }
        _ => (None, None),
    };

    Ok(quote! {
        #(#attrs)*
        #[doc = ""]
        #[doc = #info]
        #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
        #vis struct #name;

        #[allow(dead_code, trivial_numeric_casts, clippy::unnecessary_cast)]
        impl #name {
            #limits
            #step
        }

        #[allow(trivial_numeric_casts, clippy::unnecessary_cast)]
        impl ::quantbits::Codec for #name {
            type Value = #ty;
            const BITS: u32 = #bits;

            #[inline]
            fn encode(value: #ty) -> u64 {
                #encode
            }

            #[inline]
            fn decode(raw: u64) -> #ty {
                #decode
            }

            #try_encode
        }

        #stochastic_impl
        #check
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

    // `as u64` below would silently truncate larger discriminants of a `#[repr(u128)]` enum.
    let discriminant_checks = variants.iter().map(|v| {
        let negative = format!("quantbits: `{name_str}::{v}` has a negative discriminant");
        let too_large = format!("quantbits: `{name_str}::{v}` has a discriminant larger than u64::MAX");
        quote_spanned! {v.span()=>
            ::core::assert!((#name::#v as i128) >= 0, #negative);
            ::core::assert!((#name::#v as u128) <= u64::MAX as u128, #too_large);
        }
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

        impl ::quantbits::Codec for #name {
            type Value = Self;
            const BITS: u32 = <Self as #enum_trait>::BITS;

            #[inline]
            fn encode(value: Self) -> u64 {
                <Self as #enum_trait>::into_bits(value)
            }

            #[inline]
            fn decode(raw: u64) -> Self {
                <Self as #enum_trait>::from_bits(raw)
            }
        }

        const _: () = { #(#discriminant_checks)* };
        #exhaustive_check
    })
}
