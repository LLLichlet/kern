use kernc_sema::SemaContext;
use kernc_sema::def::{Def, DefId};

pub(crate) fn is_language_builtin_def(ctx: &SemaContext<'_>, def: &Def) -> bool {
    match def {
        Def::Function(function) => {
            function.is_intrinsic
                || function
                    .parent
                    .is_some_and(|parent| is_language_builtin_impl(ctx, parent))
        }
        Def::Trait(def) => def.is_builtin,
        Def::AssociatedType(def) => {
            def.parent_trait
                .is_some_and(|parent| is_builtin_trait(ctx, parent))
                || def
                    .parent_impl
                    .is_some_and(|parent| is_language_builtin_impl(ctx, parent))
        }
        Def::Impl(def) => is_language_builtin_impl(ctx, def.id),
        _ => false,
    }
}

pub(crate) fn is_language_builtin_def_id(ctx: &SemaContext<'_>, def_id: DefId) -> bool {
    ctx.defs
        .get(def_id.0 as usize)
        .is_some_and(|def| is_language_builtin_def(ctx, def))
}

pub(crate) fn is_language_builtin_impl(ctx: &SemaContext<'_>, impl_id: DefId) -> bool {
    let Some(Def::Impl(def)) = ctx.defs.get(impl_id.0 as usize) else {
        return false;
    };
    let Some(trait_type) = &def.trait_type else {
        return false;
    };
    let Some(kernc_sema::ty::TypeKind::TraitObject(trait_id, _, _)) = ctx
        .node_type(trait_type.id)
        .map(|ty| ctx.type_registry.get(ctx.type_registry.normalize(ty)))
    else {
        return false;
    };

    is_builtin_trait(ctx, *trait_id)
}

fn is_builtin_trait(ctx: &SemaContext<'_>, trait_id: DefId) -> bool {
    matches!(
        ctx.defs.get(trait_id.0 as usize),
        Some(Def::Trait(def)) if def.is_builtin
    )
}
