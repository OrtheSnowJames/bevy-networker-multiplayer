use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{
    Data, DeriveInput, Error, Expr, Fields, ItemStruct, Path, Token,
    parse::{Parse, ParseStream},
    parse_macro_input,
    punctuated::Punctuated,
    token::Comma,
};

struct SyncArgs {
    is_resource: bool,
    prefab_components: Option<Vec<Expr>>,
    resource_interval: Option<Expr>,
    resource_heartbeat: Option<Expr>,
}

enum SyncArg {
    Resource,
    Prefab(Vec<Expr>),
    Interval(Expr),
    Heartbeat(Expr),
}

impl Parse for SyncArg {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let lookahead = input.lookahead1();
        if lookahead.peek(syn::Ident) {
            let ident: syn::Ident = input.parse()?;
            if ident == "resource" {
                return Ok(Self::Resource);
            }

            if ident == "prefab" {
                let content;
                syn::parenthesized!(content in input);
                let components = Punctuated::<Expr, Comma>::parse_terminated(&content)?
                    .into_iter()
                    .collect();
                return Ok(Self::Prefab(components));
            }

            if ident == "interval" {
                input.parse::<Token![=]>()?;
                return Ok(Self::Interval(input.parse()?));
            }

            if ident == "heartbeat" {
                input.parse::<Token![=]>()?;
                return Ok(Self::Heartbeat(input.parse()?));
            }

            return Err(Error::new_spanned(
                ident,
                "unsupported #[sync(...)] argument",
            ));
        }

        Err(lookahead.error())
    }
}

impl Parse for SyncArgs {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let args = Punctuated::<SyncArg, Comma>::parse_terminated(input)?;
        let mut is_resource = false;
        let mut prefab_components = None;
        let mut resource_interval = None;
        let mut resource_heartbeat = None;

        for arg in args {
            match arg {
                SyncArg::Resource => {
                    is_resource = true;
                }
                SyncArg::Prefab(components) => {
                    prefab_components = Some(components);
                }
                SyncArg::Interval(seconds) => {
                    resource_interval = Some(seconds);
                }
                SyncArg::Heartbeat(seconds) => {
                    resource_heartbeat = Some(seconds);
                }
            }
        }

        if !is_resource && (resource_interval.is_some() || resource_heartbeat.is_some()) {
            return Err(Error::new(
                input.span(),
                "`interval` and `heartbeat` are only supported with #[sync(resource)]",
            ));
        }

        Ok(Self {
            is_resource,
            prefab_components,
            resource_interval,
            resource_heartbeat,
        })
    }
}

#[proc_macro_attribute]
pub fn sync(args: TokenStream, input: TokenStream) -> TokenStream {
    let args = match parse_sync_args(args) {
        Ok(args) => args,
        Err(error) => return error,
    };
    let item = parse_macro_input!(input as ItemStruct);
    expand_sync(item, args).into()
}

#[proc_macro_attribute]
pub fn netmsg(args: TokenStream, input: TokenStream) -> TokenStream {
    if !args.is_empty() {
        let args_tokens: proc_macro2::TokenStream = args.into();
        return Error::new_spanned(args_tokens, "#[netmsg] does not take any arguments")
            .to_compile_error()
            .into();
    }

    let item = parse_macro_input!(input as ItemStruct);
    expand_netmsg(item).into()
}

#[proc_macro_derive(PredictLinearMotion)]
pub fn derive_predict_linear_motion(input: TokenStream) -> TokenStream {
    expand_prediction_derive(input, PredictionDeriveKind::PredictLinearMotion).into()
}

#[proc_macro_derive(Velocity2d)]
pub fn derive_velocity_2d(input: TokenStream) -> TokenStream {
    expand_prediction_derive(input, PredictionDeriveKind::Velocity2d).into()
}

fn parse_sync_args(args: TokenStream) -> Result<SyncArgs, TokenStream> {
    if args.is_empty() {
        return Ok(SyncArgs {
            is_resource: false,
            prefab_components: None,
            resource_interval: None,
            resource_heartbeat: None,
        });
    }

    syn::parse::<SyncArgs>(args).map_err(|error| -> TokenStream { error.to_compile_error().into() })
}

fn expand_sync(mut item: ItemStruct, args: SyncArgs) -> proc_macro2::TokenStream {
    let ident = item.ident.clone();
    let generics = item.generics.clone();
    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();
    let prefab_components = args.prefab_components.clone();

    let mut derive_paths: Vec<Path> = Vec::new();
    let mut retained_attrs = Vec::new();

    for attr in item.attrs.into_iter() {
        if attr.path().is_ident("derive") {
            let parsed: Punctuated<Path, Token![,]> = attr
                .parse_args_with(Punctuated::<Path, Token![,]>::parse_terminated)
                .expect("failed to parse derive attributes");
            derive_paths.extend(parsed.into_iter());
        } else {
            retained_attrs.push(attr);
        }
    }

    for required in [
        syn::parse_str::<Path>("::bevy_networker_multiplayer::serde::Serialize").unwrap(),
        syn::parse_str::<Path>("::bevy_networker_multiplayer::serde::Deserialize").unwrap(),
        syn::parse_str::<Path>("Clone").unwrap(),
    ] {
        if !has_derive(&derive_paths, &required) {
            derive_paths.push(required);
        }
    }

    if args.is_resource {
        let required =
            syn::parse_str::<Path>("::bevy_networker_multiplayer::bevy::prelude::Resource")
                .unwrap();
        if !has_derive(&derive_paths, &required) {
            derive_paths.push(required);
        }
    }

    item.attrs = retained_attrs;

    let register_fn = format_ident!("__{}_register_sync", ident);
    let prefab_register_fn = format_ident!("__{}_register_prefab", ident);
    let apply_fn = format_ident!("__{}_apply_sync", ident);
    let snapshot_fn = format_ident!("__{}_snapshot_sync", ident);
    let prefab_apply_fn = format_ident!("__{}_apply_prefab", ident);
    let prefab_matches_fn = format_ident!("__{}_matches_prefab", ident);
    let follow_fn = format_ident!("__{}_follow_visual_transform", ident);
    let resource_sync_fn = format_ident!("__{}_sync_resource", ident);

    let sync_trait = if args.is_resource {
        quote! { SyncResource }
    } else {
        quote! { SyncComponent }
    };

    let register_system = if args.is_resource {
        quote! {
            app.add_systems(
                ::bevy_networker_multiplayer::bevy::prelude::Update,
                #resource_sync_fn,
            );
        }
    } else {
        quote! {
            app.add_systems(
                ::bevy_networker_multiplayer::bevy::prelude::Update,
                ::bevy_networker_multiplayer::sync::sync_component::<#ident #ty_generics>
                    .after(::bevy_networker_multiplayer::sync::assign_network_ids),
            );
        }
    };

    let follow_system = if prefab_components.is_some() && is_vec2_tuple_struct(&item) {
        quote! {
            app.add_systems(
                ::bevy_networker_multiplayer::bevy::prelude::PostUpdate,
                #follow_fn,
            );
        }
    } else {
        quote! {}
    };

    let registration = if args.is_resource {
        quote! {
            ::bevy_networker_multiplayer::inventory::submit! {
                ::bevy_networker_multiplayer::sync::ResourceRegistration {
                    type_path: concat!(module_path!(), "::", stringify!(#ident)),
                    wire_id: ::bevy_networker_multiplayer::sync::hash_type_path(concat!(module_path!(), "::", stringify!(#ident))),
                    register: #register_fn,
                    apply: #apply_fn,
                    snapshot: #snapshot_fn,
                }
            }
        }
    } else if prefab_components.is_some() {
        quote! {
            ::bevy_networker_multiplayer::inventory::submit! {
                ::bevy_networker_multiplayer::sync::ComponentRegistration {
                    type_path: concat!(module_path!(), "::", stringify!(#ident)),
                    wire_id: ::bevy_networker_multiplayer::sync::hash_type_path(concat!(module_path!(), "::", stringify!(#ident))),
                    register: #register_fn,
                    apply: #apply_fn,
                    snapshot: #snapshot_fn,
                }
            }

            ::bevy_networker_multiplayer::inventory::submit! {
                ::bevy_networker_multiplayer::sync::PrefabRegistration {
                    type_path: concat!(module_path!(), "::", stringify!(#ident)),
                    wire_id: ::bevy_networker_multiplayer::sync::hash_type_path(concat!(module_path!(), "::", stringify!(#ident))),
                    register: #prefab_register_fn,
                    matches: #prefab_matches_fn,
                    apply: #prefab_apply_fn,
                }
            }
        }
    } else {
        quote! {
            ::bevy_networker_multiplayer::inventory::submit! {
                ::bevy_networker_multiplayer::sync::ComponentRegistration {
                    type_path: concat!(module_path!(), "::", stringify!(#ident)),
                    wire_id: ::bevy_networker_multiplayer::sync::hash_type_path(concat!(module_path!(), "::", stringify!(#ident))),
                    register: #register_fn,
                    apply: #apply_fn,
                    snapshot: #snapshot_fn,
                }
            }
        }
    };

    let resource_interval = args
        .resource_interval
        .clone()
        .unwrap_or_else(|| syn::parse_quote!(0.0));
    let resource_heartbeat = args
        .resource_heartbeat
        .clone()
        .map(|heartbeat| quote! { Some((#heartbeat) as f32) })
        .unwrap_or_else(|| quote! { None });

    let resource_sync_fn_def = if args.is_resource {
        quote! {
            #[allow(non_snake_case)]
            fn #resource_sync_fn(
                time: ::bevy_networker_multiplayer::bevy::prelude::Res<
                    ::bevy_networker_multiplayer::bevy::prelude::Time,
                >,
                mut net: ::bevy_networker_multiplayer::bevy::prelude::ResMut<
                    ::bevy_networker_multiplayer::NetResource,
                >,
                resource: Option<
                    ::bevy_networker_multiplayer::bevy::prelude::Res<#ident #ty_generics>,
                >,
                mut state: ::bevy_networker_multiplayer::bevy::prelude::Local<
                    ::bevy_networker_multiplayer::sync::SyncResourceSendState,
                >,
            ) {
                ::bevy_networker_multiplayer::sync::sync_resource_with_settings::<#ident #ty_generics>(
                    &time,
                    &mut net,
                    resource,
                    &mut state,
                    ::bevy_networker_multiplayer::sync::SyncResourceSettings {
                        min_interval_seconds: (#resource_interval) as f32,
                        heartbeat_seconds: #resource_heartbeat,
                    },
                );
            }
        }
    } else {
        quote! {}
    };

    let snapshot_fn_def = if args.is_resource {
        quote! {
            #[allow(non_snake_case)]
            fn #snapshot_fn(world: &mut ::bevy_networker_multiplayer::bevy::prelude::World) -> ::std::vec::Vec<::bevy_networker_multiplayer::netres::ReplicationPacket> {
                let mut packets = ::std::vec::Vec::new();

                if let Some(resource) = world.get_resource::<#ident #ty_generics>() {
                    let bytes = ::bevy_networker_multiplayer::bincode::serde::encode_to_vec(
                        resource,
                        ::bevy_networker_multiplayer::bincode::config::standard(),
                    ).expect("failed to serialize sync resource");

                    packets.push(::bevy_networker_multiplayer::netres::ReplicationPacket::UpdateResource {
                        resource_wire_id: ::bevy_networker_multiplayer::sync::hash_type_path(concat!(module_path!(), "::", stringify!(#ident))),
                        bytes,
                    });
                }

                packets
            }
        }
    } else {
        quote! {
            #[allow(non_snake_case)]
            fn #snapshot_fn(world: &mut ::bevy_networker_multiplayer::bevy::prelude::World) -> ::std::vec::Vec<::bevy_networker_multiplayer::netres::ReplicationPacket> {
                let mut packets = ::std::vec::Vec::new();
                let component_wire_id = ::bevy_networker_multiplayer::sync::hash_type_path(concat!(module_path!(), "::", stringify!(#ident)));
                let updates = {
                    let mut updates = ::std::vec::Vec::new();
                    let mut query = world.query_filtered::<(
                        ::bevy_networker_multiplayer::bevy::prelude::Entity,
                        &::bevy_networker_multiplayer::replicated::NetworkId,
                        &#ident #ty_generics,
                    ), ::bevy_networker_multiplayer::bevy::prelude::With<::bevy_networker_multiplayer::replicated::Replicated>>();

                    for (_, network_id, component) in query.iter(world) {
                        let bytes = ::bevy_networker_multiplayer::bincode::serde::encode_to_vec(
                            component,
                            ::bevy_networker_multiplayer::bincode::config::standard(),
                        ).expect("failed to serialize sync component");
                        updates.push((network_id.0, bytes));
                    }

                    updates
                };

                for (network_id, bytes) in updates {
                    packets.push(::bevy_networker_multiplayer::netres::ReplicationPacket::UpdateComponent {
                        network_id,
                        component_wire_id,
                        sequence: ::bevy_networker_multiplayer::sync::next_component_update_sequence(world),
                        bytes,
                    });
                }

                packets
            }
        }
    };

    let prefab_apply_def = if let Some(prefab_components) = prefab_components.clone() {
        quote! {
            #[allow(non_snake_case)]
            fn #prefab_apply_fn(world: &mut ::bevy_networker_multiplayer::bevy::prelude::World, entity: ::bevy_networker_multiplayer::bevy::prelude::Entity) {
                world.entity_mut(entity).insert((#(#prefab_components),*));
            }
        }
    } else {
        quote! {}
    };

    let prefab_matches_def = if prefab_components.is_some() {
        quote! {
            #[allow(non_snake_case)]
            fn #prefab_matches_fn(world: &::bevy_networker_multiplayer::bevy::prelude::World, entity: ::bevy_networker_multiplayer::bevy::prelude::Entity) -> bool {
                world.entity(entity).contains::<#ident #ty_generics>()
            }
        }
    } else {
        quote! {}
    };

    let follow_fn_def = if prefab_components.is_some() && is_vec2_tuple_struct(&item) {
        quote! {
            #[allow(non_snake_case)]
            fn #follow_fn(
                mut query: ::bevy_networker_multiplayer::bevy::prelude::Query<
                    (
                        &#ident #ty_generics,
                        &mut ::bevy_networker_multiplayer::bevy::prelude::Transform,
                    ),
                    (
                        ::bevy_networker_multiplayer::bevy::prelude::With<
                            ::bevy_networker_multiplayer::replicated::Replicated,
                        >,
                        ::bevy_networker_multiplayer::bevy::prelude::Or<(
                            ::bevy_networker_multiplayer::bevy::prelude::Added<#ident #ty_generics>,
                            ::bevy_networker_multiplayer::bevy::prelude::Changed<#ident #ty_generics>,
                        )>,
                    ),
                >,
            ) {
                for (component, mut transform) in &mut query {
                    transform.translation.x = component.0.x;
                    transform.translation.y = component.0.y;
                }
            }
        }
    } else {
        quote! {}
    };

    let apply_fn_def = if args.is_resource {
        quote! {
            #[allow(non_snake_case)]
            fn #apply_fn(world: &mut ::bevy_networker_multiplayer::bevy::prelude::World, bytes: &[u8]) {
                ::bevy_networker_multiplayer::sync::apply_resource_update::<#ident #ty_generics>(
                    world,
                    bytes,
                );
            }
        }
    } else if prefab_components.is_some() {
        let uses_transform = is_vec2_tuple_struct(&item);
        let position_binding = if uses_transform {
            quote! {
                let position = component.0;
            }
        } else {
            quote! {}
        };
        let visual_update = if uses_transform {
            quote! {
                if let Some(mut transform) = entity.get_mut::<::bevy_networker_multiplayer::bevy::prelude::Transform>() {
                    transform.translation.x = position.x;
                    transform.translation.y = position.y;
                }
            }
        } else {
            quote! {}
        };

        quote! {
            #[allow(non_snake_case)]
            fn #apply_fn(world: &mut ::bevy_networker_multiplayer::bevy::prelude::World, entity: ::bevy_networker_multiplayer::bevy::prelude::Entity, bytes: &[u8]) {
                let (component, _): (#ident #ty_generics, usize) = ::bevy_networker_multiplayer::bincode::serde::decode_from_slice(
                    bytes,
                    ::bevy_networker_multiplayer::bincode::config::standard(),
                ).expect("failed to deserialize sync component");

                #position_binding
                let mut entity = world.entity_mut(entity);
                entity.insert(component);
                #visual_update
            }
        }
    } else {
        quote! {
            #[allow(non_snake_case)]
            fn #apply_fn(world: &mut ::bevy_networker_multiplayer::bevy::prelude::World, entity: ::bevy_networker_multiplayer::bevy::prelude::Entity, bytes: &[u8]) {
                let (component, _): (#ident #ty_generics, usize) = ::bevy_networker_multiplayer::bincode::serde::decode_from_slice(
                    bytes,
                    ::bevy_networker_multiplayer::bincode::config::standard(),
                ).expect("failed to deserialize sync component");

                world.entity_mut(entity).insert(component);
            }
        }
    };

    quote! {
        #[derive(#(#derive_paths),*)]
        #item

        impl #impl_generics ::bevy_networker_multiplayer::sync::#sync_trait for #ident #ty_generics #where_clause {
            const TYPE_PATH: &'static str = concat!(module_path!(), "::", stringify!(#ident));
            const WIRE_ID: u64 = ::bevy_networker_multiplayer::sync::hash_type_path(Self::TYPE_PATH);
        }

        #apply_fn_def

        #[allow(non_snake_case)]
        fn #register_fn(app: &mut ::bevy_networker_multiplayer::bevy::prelude::App) {
            #register_system
            #follow_system
        }

        #[allow(non_snake_case)]
        fn #prefab_register_fn(_app: &mut ::bevy_networker_multiplayer::bevy::prelude::App) {}

        #registration
        #resource_sync_fn_def
        #snapshot_fn_def
        #prefab_apply_def
        #prefab_matches_def
        #follow_fn_def
    }
}

fn expand_netmsg(item: ItemStruct) -> proc_macro2::TokenStream {
    let ident = item.ident.clone();
    let generics = item.generics.clone();
    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();

    quote! {
        #item

        impl #impl_generics ::bevy_networker_multiplayer::NetMessage for #ident #ty_generics #where_clause {
            const TYPE_PATH: &'static str = concat!(module_path!(), "::", stringify!(#ident));
            const WIRE_ID: u64 = ::bevy_networker_multiplayer::netmsg::hash_type_path(Self::TYPE_PATH);
        }
    }
}

enum PredictionDeriveKind {
    PredictLinearMotion,
    Velocity2d,
}

fn expand_prediction_derive(
    input: TokenStream,
    kind: PredictionDeriveKind,
) -> proc_macro2::TokenStream {
    let input = match syn::parse::<DeriveInput>(input) {
        Ok(input) => input,
        Err(error) => return error.to_compile_error(),
    };
    let ident = input.ident;
    let generics = input.generics;
    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();

    let field_error = match kind {
        PredictionDeriveKind::PredictLinearMotion => {
            "PredictLinearMotion can only be derived for tuple structs with one Vec2 field"
        }
        PredictionDeriveKind::Velocity2d => {
            "Velocity2d can only be derived for tuple structs with one Vec2 field"
        }
    };

    match input.data {
        Data::Struct(data) => match data.fields {
            Fields::Unnamed(fields)
                if fields.unnamed.len() == 1 && is_vec2_type(&fields.unnamed[0].ty) => {}
            _ => return Error::new_spanned(ident, field_error).to_compile_error(),
        },
        _ => return Error::new_spanned(ident, field_error).to_compile_error(),
    }

    match kind {
        PredictionDeriveKind::PredictLinearMotion => quote! {
            impl #impl_generics ::bevy_networker_multiplayer::prediction::PredictLinearMotion
                for #ident #ty_generics #where_clause
            {
                fn predicted_position(&self) -> ::bevy_networker_multiplayer::bevy::prelude::Vec2 {
                    self.0
                }

                fn set_predicted_position(
                    &mut self,
                    position: ::bevy_networker_multiplayer::bevy::prelude::Vec2,
                ) {
                    self.0 = position;
                }
            }
        },
        PredictionDeriveKind::Velocity2d => quote! {
            impl #impl_generics ::bevy_networker_multiplayer::prediction::Velocity2d
                for #ident #ty_generics #where_clause
            {
                fn velocity_2d(&self) -> ::bevy_networker_multiplayer::bevy::prelude::Vec2 {
                    self.0
                }
            }
        },
    }
}

fn has_derive(existing: &[Path], required: &Path) -> bool {
    let Some(required_ident) = required.segments.last().map(|segment| &segment.ident) else {
        return false;
    };

    existing
        .iter()
        .any(|path| path.segments.last().map(|segment| &segment.ident) == Some(required_ident))
}

fn is_vec2_tuple_struct(item: &ItemStruct) -> bool {
    matches!(
        &item.fields,
        Fields::Unnamed(fields) if fields.unnamed.len() == 1 && is_vec2_type(&fields.unnamed[0].ty)
    )
}

fn is_vec2_type(ty: &syn::Type) -> bool {
    match ty {
        syn::Type::Path(type_path) => type_path
            .path
            .segments
            .last()
            .map(|segment| segment.ident == "Vec2")
            .unwrap_or(false),
        _ => false,
    }
}
