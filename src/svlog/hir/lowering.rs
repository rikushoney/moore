// Copyright (c) 2016-2019 Fabian Schuiki

//! Lowering of AST nodes to HIR nodes.

use crate::{ast_map::AstNode, crate_prelude::*, hir::HirNode};
use bit_vec::BitVec;
use num::BigInt;
use std::collections::HashMap;

/// A hint about how a node should be lowered to HIR.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Hint {
    /// Lower as type.
    Type,
    /// Lower as expression.
    Expr,
}

pub(crate) fn hir_of<'gcx>(cx: &impl Context<'gcx>, node_id: NodeId) -> Result<HirNode<'gcx>> {
    let ast = cx.ast_of(node_id)?;

    #[allow(unreachable_patterns)]
    match ast {
        AstNode::Module(m) => lower_module(cx, node_id, m),
        AstNode::Port(p) => lower_port(cx, node_id, p),
        AstNode::Type(ty) => lower_type(cx, node_id, ty),
        AstNode::TypeOrExpr(&ast::TypeOrExpr::Type(ref ty)) => lower_type(cx, node_id, ty),
        AstNode::TypeOrExpr(&ast::TypeOrExpr::Expr(ref expr)) => lower_expr(cx, node_id, expr),
        // AstNode::TypeOrExpr(&ast::TypeOrExpr::Type(ref ty))
        //     if cx.lowering_hint(node_id) == Some(Hint::Type) =>
        // {
        //     lower_type(cx, node_id, ty)
        // }
        // AstNode::TypeOrExpr(&ast::TypeOrExpr::Expr(ref expr))
        //     if cx.lowering_hint(node_id) == Some(Hint::Expr) =>
        // {
        //     lower_expr(cx, node_id, expr)
        // }
        AstNode::Expr(expr) => lower_expr(cx, node_id, expr),
        AstNode::InstTarget(ast) => {
            let mut named_params = vec![];
            let mut pos_params = vec![];
            let mut is_pos = true;
            for param in &ast.params {
                let value_id = cx.map_ast_with_parent(AstNode::TypeOrExpr(&param.expr), node_id);
                if let Some(name) = param.name {
                    is_pos = false;
                    named_params.push((
                        param.span,
                        Spanned::new(name.name, name.span),
                        Some(value_id),
                    ));
                } else {
                    if !is_pos {
                        cx.emit(
                            DiagBuilder2::warning("positional parameters must appear before named")
                                .span(param.span)
                                .add_note(format!(
                                    "assuming this refers to argument #{}",
                                    pos_params.len() + 1
                                )),
                        );
                    }
                    pos_params.push((param.span, Some(value_id)));
                }
            }
            let hir = hir::InstTarget {
                id: node_id,
                name: Spanned::new(ast.target.name, ast.target.span),
                span: ast.span,
                pos_params,
                named_params,
            };
            Ok(HirNode::InstTarget(cx.arena().alloc_hir(hir)))
        }
        AstNode::Inst(inst, target_id) => {
            let mut named_ports = vec![];
            let mut pos_ports = vec![];
            let mut has_wildcard_port = false;
            let mut is_pos = true;
            for port in &inst.conns {
                match port.kind {
                    ast::PortConnKind::Auto => has_wildcard_port = true,
                    ast::PortConnKind::Named(name, ref mode) => {
                        let name = Spanned::new(name.name, name.span);
                        is_pos = false;
                        let value_id = match *mode {
                            ast::PortConnMode::Auto => Some(cx.resolve_upwards_or_error(
                                name,
                                cx.parent_node_id(node_id).unwrap(),
                            )?),
                            ast::PortConnMode::Unconnected => None,
                            ast::PortConnMode::Connected(ref expr) => {
                                Some(cx.map_ast_with_parent(AstNode::Expr(expr), node_id))
                            }
                        };
                        named_ports.push((port.span, name, value_id));
                    }
                    ast::PortConnKind::Positional(ref expr) => {
                        if !is_pos {
                            cx.emit(
                                DiagBuilder2::warning("positional port must appear before named")
                                    .span(port.span)
                                    .add_note(format!(
                                        "assuming this refers to argument #{}",
                                        pos_ports.len() + 1
                                    )),
                            );
                        }
                        let value_id = cx.map_ast_with_parent(AstNode::Expr(expr), node_id);
                        pos_ports.push((port.span, Some(value_id)));
                    }
                }
            }
            let hir = hir::Inst {
                id: node_id,
                name: Spanned::new(inst.name.name, inst.name.span),
                span: inst.span,
                target: target_id,
                named_ports,
                pos_ports,
                has_wildcard_port,
                dummy: Default::default(),
            };
            Ok(HirNode::Inst(cx.arena().alloc_hir(hir)))
        }
        AstNode::TypeParam(param, decl) => {
            let hir = hir::TypeParam {
                id: node_id,
                name: Spanned::new(decl.name.name, decl.name.span),
                span: Span::union(param.span, decl.span),
                local: param.local,
                default: decl
                    .ty
                    .as_ref()
                    .map(|ty| cx.map_ast_with_parent(AstNode::Type(ty), node_id)),
            };
            Ok(HirNode::TypeParam(cx.arena().alloc_hir(hir)))
        }
        AstNode::ValueParam(param, decl) => {
            let hir = hir::ValueParam {
                id: node_id,
                name: Spanned::new(decl.name.name, decl.name.span),
                span: Span::union(param.span, decl.span),
                local: param.local,
                ty: cx.map_ast_with_parent(AstNode::Type(&decl.ty), node_id),
                default: decl
                    .expr
                    .as_ref()
                    .map(|expr| cx.map_ast_with_parent(AstNode::Expr(expr), node_id)),
            };
            Ok(HirNode::ValueParam(cx.arena().alloc_hir(hir)))
        }
        AstNode::VarDecl(name, decl, ty) => {
            let hir = hir::VarDecl {
                id: node_id,
                name: Spanned::new(name.name, name.name_span),
                span: Span::union(name.span, decl.span),
                ty: ty,
                init: name
                    .init
                    .as_ref()
                    .map(|expr| cx.map_ast_with_parent(AstNode::Expr(expr), node_id)),
            };
            Ok(HirNode::VarDecl(cx.arena().alloc_hir(hir)))
        }
        AstNode::NetDecl(name, decl, ty) => {
            // TODO(fschuiki): Map this to something different than a variable.
            let hir = hir::VarDecl {
                id: node_id,
                name: Spanned::new(name.name, name.name_span),
                span: Span::union(name.span, decl.span),
                ty: ty,
                init: name
                    .init
                    .as_ref()
                    .map(|expr| cx.map_ast_with_parent(AstNode::Expr(expr), node_id)),
            };
            Ok(HirNode::VarDecl(cx.arena().alloc_hir(hir)))
        }
        AstNode::Proc(prok) => {
            let hir = hir::Proc {
                id: node_id,
                span: prok.span,
                kind: prok.kind,
                stmt: cx.map_ast_with_parent(AstNode::Stmt(&prok.stmt), node_id),
            };
            Ok(HirNode::Proc(cx.arena().alloc_hir(hir)))
        }
        AstNode::Stmt(stmt) => {
            let kind = match stmt.data {
                ast::NullStmt => hir::StmtKind::Null,
                ast::SequentialBlock(ref stmts) => {
                    let mut next_rib = node_id;
                    hir::StmtKind::Block(
                        stmts
                            .iter()
                            .map(|stmt| {
                                let id = cx.map_ast_with_parent(AstNode::Stmt(stmt), next_rib);
                                next_rib = id;
                                id
                            })
                            .collect(),
                    )
                }
                ast::BlockingAssignStmt {
                    ref lhs,
                    ref rhs,
                    op,
                } => hir::StmtKind::Assign {
                    lhs: cx.map_ast_with_parent(AstNode::Expr(lhs), node_id),
                    rhs: cx.map_ast_with_parent(AstNode::Expr(rhs), node_id),
                    kind: hir::AssignKind::Block(op),
                },
                ast::TimedStmt(ref control, ref inner_stmt) => {
                    let control = match *control {
                        ast::TimingControl::Delay(ref dc) => hir::TimingControl::Delay(
                            cx.map_ast_with_parent(AstNode::Expr(&dc.expr), node_id),
                        ),
                        ast::TimingControl::Event(ref ec) => match ec.data {
                            ast::EventControlData::Implicit => hir::TimingControl::ImplicitEvent,
                            ast::EventControlData::Expr(ref expr) => {
                                hir::TimingControl::ExplicitEvent(
                                    cx.map_ast_with_parent(AstNode::EventExpr(expr), node_id),
                                )
                            }
                        },
                        _ => {
                            debug!("{:#?}", stmt);
                            return cx.unimp_msg("lowering of timing control", stmt);
                        }
                    };
                    hir::StmtKind::Timed {
                        control,
                        stmt: cx.map_ast_with_parent(AstNode::Stmt(inner_stmt), node_id),
                    }
                }
                ast::IfStmt {
                    ref cond,
                    ref main_stmt,
                    ref else_stmt,
                    ..
                } => hir::StmtKind::If {
                    cond: cx.map_ast_with_parent(AstNode::Expr(cond), node_id),
                    main_stmt: cx.map_ast_with_parent(AstNode::Stmt(main_stmt), node_id),
                    else_stmt: else_stmt
                        .as_ref()
                        .map(|else_stmt| cx.map_ast_with_parent(AstNode::Stmt(else_stmt), node_id)),
                },
                ast::ExprStmt(ref expr) => {
                    hir::StmtKind::Expr(cx.map_ast_with_parent(AstNode::Expr(expr), node_id))
                }
                ast::ForeverStmt(ref body) => hir::StmtKind::Loop {
                    kind: hir::LoopKind::Forever,
                    body: cx.map_ast_with_parent(AstNode::Stmt(body), node_id),
                },
                ast::RepeatStmt(ref count, ref body) => hir::StmtKind::Loop {
                    kind: hir::LoopKind::Repeat(
                        cx.map_ast_with_parent(AstNode::Expr(count), node_id),
                    ),
                    body: cx.map_ast_with_parent(AstNode::Stmt(body), node_id),
                },
                ast::WhileStmt(ref cond, ref body) => hir::StmtKind::Loop {
                    kind: hir::LoopKind::While(
                        cx.map_ast_with_parent(AstNode::Expr(cond), node_id),
                    ),
                    body: cx.map_ast_with_parent(AstNode::Stmt(body), node_id),
                },
                ast::DoStmt(ref body, ref cond) => hir::StmtKind::Loop {
                    kind: hir::LoopKind::Do(cx.map_ast_with_parent(AstNode::Expr(cond), node_id)),
                    body: cx.map_ast_with_parent(AstNode::Stmt(body), node_id),
                },
                ast::ForStmt(ref init, ref cond, ref step, ref body) => {
                    let init = cx.map_ast_with_parent(AstNode::Stmt(init), node_id);
                    let cond = cx.map_ast_with_parent(AstNode::Expr(cond), init);
                    let step = cx.map_ast_with_parent(AstNode::Expr(step), init);
                    hir::StmtKind::Loop {
                        kind: hir::LoopKind::For(init, cond, step),
                        body: cx.map_ast_with_parent(AstNode::Stmt(body), init),
                    }
                }
                ast::VarDeclStmt(ref decls) => {
                    let mut stmts = vec![];
                    let parent = cx.parent_node_id(node_id).unwrap();
                    let rib = alloc_var_decl(cx, decls, parent, &mut stmts);
                    hir::StmtKind::InlineGroup { stmts, rib }
                }
                ast::NonblockingAssignStmt {
                    ref lhs,
                    ref rhs,
                    ref delay,
                    ..
                } => hir::StmtKind::Assign {
                    lhs: cx.map_ast_with_parent(AstNode::Expr(lhs), node_id),
                    rhs: cx.map_ast_with_parent(AstNode::Expr(rhs), node_id),
                    kind: match *delay {
                        Some(ref dc) => hir::AssignKind::NonblockDelay(
                            cx.map_ast_with_parent(AstNode::Expr(&dc.expr), node_id),
                        ),
                        None => hir::AssignKind::Nonblock,
                    },
                },
                ast::CaseStmt {
                    ref expr,
                    mode: ast::CaseMode::Normal,
                    ref items,
                    kind,
                    ..
                } => {
                    let expr = cx.map_ast_with_parent(AstNode::Expr(expr), node_id);
                    let mut ways = vec![];
                    let mut default = None;
                    for item in items {
                        match *item {
                            ast::CaseItem::Default(ref stmt) => {
                                if default.is_none() {
                                    default =
                                        Some(cx.map_ast_with_parent(AstNode::Stmt(stmt), node_id));
                                } else {
                                    cx.emit(
                                        DiagBuilder2::error("multiple default cases")
                                            .span(stmt.human_span()),
                                    );
                                }
                            }
                            ast::CaseItem::Expr(ref exprs, ref stmt) => ways.push((
                                exprs
                                    .iter()
                                    .map(|expr| {
                                        cx.map_ast_with_parent(AstNode::Expr(expr), node_id)
                                    })
                                    .collect(),
                                cx.map_ast_with_parent(AstNode::Stmt(stmt), node_id),
                            )),
                        }
                    }
                    hir::StmtKind::Case {
                        expr,
                        ways,
                        default,
                        kind,
                    }
                }
                ast::AssertionStmt { .. } => {
                    warn!("ignoring unsupported assertion `{}`", stmt.span.extract());
                    hir::StmtKind::Null
                }
                _ => {
                    error!("{:#?}", stmt);
                    return cx.unimp_msg("lowering of", stmt);
                }
            };
            let hir = hir::Stmt {
                id: node_id,
                label: stmt.label.map(|n| Spanned::new(n, stmt.span)), // this is horrible...
                span: stmt.span,
                kind: kind,
            };
            Ok(HirNode::Stmt(cx.arena().alloc_hir(hir)))
        }
        AstNode::EventExpr(expr) => {
            let mut events = vec![];
            lower_event_expr(cx, expr, node_id, &mut events, &mut vec![])?;
            let hir = hir::EventExpr {
                id: node_id,
                span: expr.span(),
                events,
            };
            Ok(HirNode::EventExpr(cx.arena().alloc_hir(hir)))
        }
        AstNode::GenIf(gen) => {
            let cond = cx.map_ast_with_parent(AstNode::Expr(&gen.cond), node_id);
            let main_body = lower_module_block(cx, node_id, &gen.main_block.items)?;
            let else_body = match gen.else_block {
                Some(ref else_block) => Some(lower_module_block(cx, node_id, &else_block.items)?),
                None => None,
            };
            let hir = hir::Gen {
                id: node_id,
                span: gen.span(),
                kind: hir::GenKind::If {
                    cond,
                    main_body,
                    else_body,
                },
            };
            Ok(HirNode::Gen(cx.arena().alloc_hir(hir)))
        }
        AstNode::GenFor(gen) => {
            let init = alloc_genvar_init(cx, &gen.init, node_id)?;
            let rib = *init.last().unwrap();
            let cond = cx.map_ast_with_parent(AstNode::Expr(&gen.cond), rib);
            let step = cx.map_ast_with_parent(AstNode::Expr(&gen.step), rib);
            let body = lower_module_block(cx, rib, &gen.block.items)?;
            let hir = hir::Gen {
                id: node_id,
                span: gen.span(),
                kind: hir::GenKind::For {
                    init,
                    cond,
                    step,
                    body,
                },
            };
            Ok(HirNode::Gen(cx.arena().alloc_hir(hir)))
        }
        AstNode::GenvarDecl(decl) => {
            let hir = hir::GenvarDecl {
                id: node_id,
                span: decl.span(),
                name: Spanned::new(decl.name, decl.name_span),
                init: decl
                    .init
                    .as_ref()
                    .map(|init| cx.map_ast_with_parent(AstNode::Expr(init), node_id)),
            };
            Ok(HirNode::GenvarDecl(cx.arena().alloc_hir(hir)))
        }
        AstNode::Typedef(def) => {
            let hir = hir::Typedef {
                id: node_id,
                span: def.span(),
                name: Spanned::new(def.name.name, def.name.span),
                ty: cx.map_ast_with_parent(
                    AstNode::Type(&def.ty),
                    cx.parent_node_id(node_id).unwrap(),
                ),
            };
            Ok(HirNode::Typedef(cx.arena().alloc_hir(hir)))
        }
        AstNode::ContAssign(_, lhs, rhs) => {
            let hir = hir::Assign {
                id: node_id,
                span: Span::union(lhs.span(), rhs.span()),
                lhs: cx.map_ast_with_parent(AstNode::Expr(lhs), node_id),
                rhs: cx.map_ast_with_parent(AstNode::Expr(rhs), node_id),
            };
            Ok(HirNode::Assign(cx.arena().alloc_hir(hir)))
        }
        AstNode::StructMember(name, decl, ty) => {
            let hir = hir::VarDecl {
                id: node_id,
                name: Spanned::new(name.name, name.name_span),
                span: Span::union(name.span, decl.span),
                ty: ty,
                init: name
                    .init
                    .as_ref()
                    .map(|expr| cx.map_ast_with_parent(AstNode::Expr(expr), ty)),
            };
            Ok(HirNode::VarDecl(cx.arena().alloc_hir(hir)))
        }
        AstNode::Package(p) => lower_package(cx, node_id, p),
        AstNode::EnumVariant(var, decl, index) => {
            let hir = hir::EnumVariant {
                id: node_id,
                name: Spanned::new(var.name.name, var.name.span),
                span: var.name.span,
                enum_id: decl,
                index,
                value: var
                    .value
                    .as_ref()
                    .map(|expr| cx.map_ast_with_parent(AstNode::Expr(expr), decl)),
            };
            Ok(HirNode::EnumVariant(cx.arena().alloc_hir(hir)))
        }
        AstNode::Import(import) => unreachable!("import should never be lowered: {:#?}", import),
        _ => {
            error!("{:#?}", ast);
            cx.unimp_msg("lowering of", &ast)
        }
    }
}

/// Lower a module to HIR.
///
/// This allocates node IDs to everything in the module and registers AST nodes
/// for each ID.
fn lower_module<'gcx>(
    cx: &impl Context<'gcx>,
    node_id: NodeId,
    ast: &'gcx ast::ModDecl,
) -> Result<HirNode<'gcx>> {
    let mut next_rib = node_id;

    // Allocate parameters.
    let mut params = Vec::new();
    for param in &ast.params {
        next_rib = alloc_param_decl(cx, param, next_rib, &mut params);
    }

    // Lower the module's ports.
    lower_module_ports(cx, &ast.ports, &ast.items)?;

    // Allocate ports.
    let mut ports = Vec::new();
    for port in &ast.ports {
        match *port {
            ast::Port::Named { .. } => {
                let id = cx.map_ast(AstNode::Port(port));
                cx.set_parent(id, next_rib);
                next_rib = id;
                ports.push(id);
            }
            _ => return cx.unimp(port),
        }
    }

    // Allocate items.
    let block = lower_module_block(cx, next_rib, &ast.items)?;

    let hir = hir::Module {
        id: node_id,
        name: Spanned::new(ast.name, ast.name_span),
        span: ast.span,
        ports: cx.arena().alloc_ids(ports),
        params: cx.arena().alloc_ids(params),
        block,
    };
    Ok(HirNode::Module(cx.arena().alloc_hir(hir)))
}

/// Lower the ports of a module to HIR.
///
/// This is a fairly complex process due to the many degrees of freedom in SV.
/// Mainly we identify if the module uses an ANSI or non-ANSI style and then go
/// ahead and create the external and internal views of the ports.
fn lower_module_ports<'gcx>(
    cx: &impl Context<'gcx>,
    ast_ports: &'gcx [ast::Port],
    ast_items: &'gcx [ast::HierarchyItem],
) -> Result<()> {
    // First determined if the module uses ANSI or non-ANSI style. We do this by
    // Determining whether the first port has type, sign, and direction omitted.
    // If it has, the ports are declared in non-ANSI style.
    let (nonansi, first_span) = {
        let first = match ast_ports.first() {
            Some(p) => p,
            None => return Ok(()),
        };
        let nonansi = match *first {
            ast::Port::Explicit { ref dir, .. } if dir.is_none() => true,
            ast::Port::Named {
                ref dir,
                ref kind,
                ref ty,
                ref dims,
                ref expr,
                ..
            } if dir.is_none()
                && kind.is_none()
                && dims.is_empty()
                && expr.is_none()
                && ty.data == ast::ImplicitType
                && ty.sign == ast::TypeSign::None
                && ty.dims.is_empty() =>
            {
                true
            }
            ast::Port::Implicit(_) => true,
            _ => false,
        };
        (nonansi, first.span())
    };
    debug!(
        "Module uses {} style",
        if nonansi { "non-ANSI" } else { "ANSI" }
    );

    // Create the external and internal port views.
    let partial_ports = match nonansi {
        true => lower_module_ports_nonansi(cx, ast_ports, ast_items, first_span),
        false => lower_module_ports_ansi(cx, ast_ports, ast_items, first_span),
    }?;

    Ok(())
}

/// Lower the non-ANSI ports of a module.
fn lower_module_ports_nonansi<'gcx>(
    cx: &impl Context<'gcx>,
    ast_ports: &'gcx [ast::Port],
    ast_items: &'gcx [ast::HierarchyItem],
    first_span: Span,
) -> Result<()> {
    let mut failed = false;

    // As a first step, collect the ports declared inside the module body. These
    // will form the internal view of the ports.
    trace!("Gathering ports inside module body");
    let mut decl_order = vec![];
    let mut decls = HashMap::new();
    for item in ast_items {
        let ast = match item {
            ast::HierarchyItem::PortDecl(pd) => pd,
            _ => continue,
        };
        trace!("Found {:#?}", ast);
        for name in &ast.names {
            let data = PartialNonAnsiPort {
                span: name.name_span,
                name: name.name,
                kind: ast.kind,
                dir: ast.dir,
                ty: &ast.ty.data,
                sign: ast.ty.sign,
                packed_dims: &ast.ty.dims,
                unpacked_dims: &name.dims,
                default: name.init.as_ref(),
                var_decl: None,
                net_decl: None,
            };
            trace!("Producing {:#?}", data);
            if let Some(prev) = decls.insert(data.name, data) {
                cx.emit(
                    DiagBuilder2::error(format!("port `{}` declared multiple times", name.name))
                        .span(name.name_span)
                        .add_note("previous declaration was here:")
                        .span(prev.span),
                );
                failed = true;
            } else {
                decl_order.push(name.name);
            }
        }
    }

    // As a second step, collect the variable and net declarations inside the
    // module body which further specify a port.
    for item in ast_items {
        match item {
            ast::HierarchyItem::VarDecl(vd) => {
                for name in &vd.names {
                    let entry = match decls.get_mut(&name.name) {
                        Some(e) => e,
                        None => continue,
                    };
                    if let Some(prev) = std::mem::replace(&mut entry.var_decl, Some((vd, name))) {
                        cx.emit(
                            DiagBuilder2::error(format!(
                                "port variable `{}` declared multiple times",
                                name.name
                            ))
                            .span(name.name_span)
                            .add_note("previous declaration was here:")
                            .span(prev.1.name_span),
                        );
                        failed = true;
                    }
                }
            }
            ast::HierarchyItem::NetDecl(nd) => {
                for name in &nd.names {
                    let entry = match decls.get_mut(&name.name) {
                        Some(e) => e,
                        None => continue,
                    };
                    if let Some(prev) = std::mem::replace(&mut entry.net_decl, Some((nd, name))) {
                        cx.emit(
                            DiagBuilder2::error(format!(
                                "port net `{}` declared multiple times",
                                name.name
                            ))
                            .span(name.name_span)
                            .add_note("previous declaration was here:")
                            .span(prev.1.name_span),
                        );
                        failed = true;
                    }
                }
            }
            _ => continue,
        }
    }

    // As a third step, merge the port declarations with the optional variable
    // and net declarations.
    for name in &decl_order {
        let port = decls.get_mut(name).unwrap();

        // Check if the port is already complete, that is, already has a net or
        // variable type. In that case it's an error to provide an additional
        // variable or net declaration that goes with it.
        if port.kind.is_some() || *port.ty != ast::ImplicitType {
            for span in port
                .var_decl
                .iter()
                .map(|x| x.1.span)
                .chain(port.net_decl.iter().map(|x| x.1.span))
            {
                cx.emit(
                    DiagBuilder2::error(format!(
                        "port `{}` is complete; additional declaration forbidden",
                        port.name
                    ))
                    .span(span)
                    .add_note(
                        "Port already has a net/variable type. \
                        Cannot declare an additional net/variable with the same \
                        name.",
                    )
                    .add_note("Port declaration was here:")
                    .span(port.span),
                );
                failed = true;
            }
            port.var_decl = None;
            port.net_decl = None;
        }

        // Extract additional details of the port from optional variable and net
        // declarations.
        let (add_span, add_ty, add_sign, add_packed, add_unpacked) =
            match (port.var_decl, port.net_decl) {
                // Inherit details from a variable declaration.
                (Some(vd), None) => {
                    // TODO: Pretty sure that this can never happen, since a port
                    // which already provides this information is considered
                    // complete.
                    if port.kind.is_some() && port.kind != Some(ast::PortKind::Var) {
                        cx.emit(
                            DiagBuilder2::error(format!(
                                "net port `{}` redeclared as variable",
                                port.name
                            ))
                            .span(vd.1.span)
                            .add_note("Port declaration was here:")
                            .span(port.span),
                        );
                        failed = true;
                    }
                    port.kind = Some(ast::PortKind::Var);
                    (
                        vd.1.name_span,
                        &vd.0.ty.data,
                        vd.0.ty.sign,
                        &vd.0.ty.dims,
                        &vd.1.dims,
                    )
                }
                // Inherit details from a net declaration.
                (None, Some(nd)) => {
                    // TODO: Pretty sure that this can never happen, since a port
                    // which already provides this information is considered
                    // complete.
                    if port.kind.is_some() && port.kind == Some(ast::PortKind::Var) {
                        cx.emit(
                            DiagBuilder2::error(format!(
                                "variable port `{}` redeclared as net",
                                port.name
                            ))
                            .span(nd.1.span)
                            .add_note("Port declaration was here:")
                            .span(port.span),
                        );
                        failed = true;
                    }
                    port.kind = Some(ast::PortKind::Net(nd.0.net_type));
                    (
                        nd.1.name_span,
                        &nd.0.ty.data,
                        nd.0.ty.sign,
                        &nd.0.ty.dims,
                        &nd.1.dims,
                    )
                }
                // Handle the case where both are present.
                (Some(vd), Some(nd)) => {
                    cx.emit(
                        DiagBuilder2::error(format!(
                            "port `{}` doubly declared as variable and net",
                            port.name
                        ))
                        .span(vd.1.span)
                        .span(nd.1.span)
                        .add_note("Port declaration was here:")
                        .span(port.span),
                    );
                    failed = true;
                    continue;
                }
                // Otherwise we keep things as they are.
                (None, None) => continue,
            };

        // Merge the sign.
        match (port.sign, add_sign) {
            (a, b) if a == b => port.sign = a,
            (a, ast::TypeSign::None) => port.sign = a,
            (ast::TypeSign::None, b) => port.sign = b,
            (a, b) => {
                cx.emit(
                    DiagBuilder2::error(format!("port `{}` has contradicting signs", port.name))
                        .span(port.span)
                        .span(add_span),
                );
            }
        };

        trace!("Merging type {:#?}", add_ty);
        trace!("Merging packed dims {:#?}", add_packed);
        trace!("Merging unpacked dims {:#?}", add_unpacked);
    }

    // As a fourth step, go through the ports themselves and pair them up with
    // declarations inside the module body. This forms the external view of the
    // ports.
    // TODO

    Ok(())
}

#[derive(Debug)]
struct PartialNonAnsiPort<'a> {
    span: Span,
    name: Name,
    dir: ast::PortDir,
    kind: Option<ast::PortKind>,
    ty: &'a ast::TypeData,
    sign: ast::TypeSign,
    packed_dims: &'a [ast::TypeDim],
    unpacked_dims: &'a [ast::TypeDim],
    default: Option<&'a ast::Expr>,
    var_decl: Option<(&'a ast::VarDecl, &'a ast::VarDeclName)>,
    net_decl: Option<(&'a ast::NetDecl, &'a ast::VarDeclName)>,
}

/// Lower the ANSI ports of a module.
fn lower_module_ports_ansi<'gcx>(
    cx: &impl Context<'gcx>,
    ast_ports: &'gcx [ast::Port],
    ast_items: &'gcx [ast::HierarchyItem],
    first_span: Span,
) -> Result<()> {
    Ok(())
}

fn lower_module_block<'gcx>(
    cx: &impl Context<'gcx>,
    parent_rib: NodeId,
    items: impl IntoIterator<Item = &'gcx ast::HierarchyItem>,
) -> Result<hir::ModuleBlock> {
    let mut next_rib = parent_rib;
    let mut insts = Vec::new();
    let mut decls = Vec::new();
    let mut procs = Vec::new();
    let mut gens = Vec::new();
    let mut params = Vec::new();
    let mut assigns = Vec::new();
    for item in items {
        match *item {
            ast::HierarchyItem::Inst(ref inst) => {
                let target_id = cx.map_ast_with_parent(AstNode::InstTarget(inst), next_rib);
                next_rib = target_id;
                trace!(
                    "instantiation target `{}` => {:?}",
                    inst.target.name,
                    target_id
                );
                for inst in &inst.names {
                    let inst_id = cx.map_ast_with_parent(AstNode::Inst(inst, target_id), next_rib);
                    trace!("instantiation `{}` => {:?}", inst.name.name, inst_id);
                    next_rib = inst_id;
                    insts.push(inst_id);
                }
            }
            ast::HierarchyItem::VarDecl(ref decl) => {
                next_rib = alloc_var_decl(cx, decl, next_rib, &mut decls);
            }
            ast::HierarchyItem::NetDecl(ref decl) => {
                next_rib = alloc_net_decl(cx, decl, next_rib, &mut decls);
            }
            ast::HierarchyItem::Procedure(ref prok) => {
                let id = cx.map_ast_with_parent(AstNode::Proc(prok), next_rib);
                next_rib = id;
                procs.push(id);
            }
            ast::HierarchyItem::GenerateIf(ref gen) => {
                let id = cx.map_ast_with_parent(AstNode::GenIf(gen), next_rib);
                next_rib = id;
                gens.push(id);
            }
            ast::HierarchyItem::GenerateFor(ref gen) => {
                let id = cx.map_ast_with_parent(AstNode::GenFor(gen), next_rib);
                next_rib = id;
                gens.push(id);
            }
            ast::HierarchyItem::ParamDecl(ref param) => {
                next_rib = alloc_param_decl(cx, param, next_rib, &mut params);
            }
            ast::HierarchyItem::Typedef(ref def) => {
                let id = cx.map_ast_with_parent(AstNode::Typedef(def), next_rib);
                next_rib = id;
            }
            ast::HierarchyItem::ContAssign(ref assign) => {
                for &(ref lhs, ref rhs) in &assign.assignments {
                    let id =
                        cx.map_ast_with_parent(AstNode::ContAssign(assign, lhs, rhs), next_rib);
                    next_rib = id;
                    assigns.push(id);
                }
            }
            ast::HierarchyItem::ImportDecl(ref decl) => {
                for item in &decl.items {
                    let id = cx.map_ast_with_parent(AstNode::Import(item), next_rib);
                    next_rib = id;
                }
            }
            // _ => return cx.unimp_msg("lowering of", item),
            _ => warn!("skipping unsupported {:?}", item),
        }
    }
    Ok(hir::ModuleBlock {
        insts,
        decls,
        procs,
        gens,
        params,
        assigns,
    })
}

fn lower_port<'gcx>(
    cx: &impl Context<'gcx>,
    node_id: NodeId,
    ast: &'gcx ast::Port,
) -> Result<HirNode<'gcx>> {
    let parent = cx.parent_node_id(node_id).unwrap();
    let hir = match *ast {
        ast::Port::Named {
            span,
            name,
            dir,
            ref ty,
            ref expr,
            ..
        } => hir::Port {
            id: node_id,
            name: Spanned::new(name.name, name.span),
            span: span,
            dir: dir.expect("port missing direction"),
            ty: cx.map_ast_with_parent(AstNode::Type(ty), parent),
            default: expr
                .as_ref()
                .map(|expr| cx.map_ast_with_parent(AstNode::Expr(expr), parent)),
        },
        _ => return cx.unimp(ast),
    };
    Ok(HirNode::Port(cx.arena().alloc_hir(hir)))
}

fn lower_type<'gcx>(
    cx: &impl Context<'gcx>,
    node_id: NodeId,
    ty: &'gcx ast::Type,
) -> Result<HirNode<'gcx>> {
    let mut kind = match ty.data {
        ast::ImplicitType => hir::TypeKind::Implicit,
        ast::VoidType => hir::TypeKind::Builtin(hir::BuiltinType::Void),
        ast::BitType => hir::TypeKind::Builtin(hir::BuiltinType::Bit),
        ast::RegType => hir::TypeKind::Builtin(hir::BuiltinType::Logic),
        ast::LogicType => hir::TypeKind::Builtin(hir::BuiltinType::Logic),
        ast::ByteType => hir::TypeKind::Builtin(hir::BuiltinType::Byte),
        ast::ShortIntType => hir::TypeKind::Builtin(hir::BuiltinType::ShortInt),
        ast::IntType => hir::TypeKind::Builtin(hir::BuiltinType::Int),
        ast::IntegerType => hir::TypeKind::Builtin(hir::BuiltinType::Integer),
        ast::LongIntType => hir::TypeKind::Builtin(hir::BuiltinType::LongInt),
        ast::StringType => hir::TypeKind::Builtin(hir::BuiltinType::String),
        ast::TimeType => hir::TypeKind::Builtin(hir::BuiltinType::Time),
        ast::NamedType(name) => hir::TypeKind::Named(Spanned::new(name.name, name.span)),
        ast::StructType { ref members, .. } => {
            let mut fields = vec![];
            let mut next_rib = node_id;
            for member in members {
                next_rib = alloc_struct_member(cx, member, next_rib, &mut fields);
            }
            hir::TypeKind::Struct(fields)
        }
        ast::ScopedType {
            ref ty,
            member: false,
            name,
        } => hir::TypeKind::Scope(
            cx.map_ast_with_parent(AstNode::Type(ty.as_ref()), node_id),
            Spanned::new(name.name, name.span),
        ),
        ast::EnumType(ref repr_ty, ref names) => {
            let mut next_rib = node_id;
            let ty = match repr_ty {
                Some(ref ty) => {
                    next_rib = cx.map_ast_with_parent(AstNode::Type(ty), next_rib);
                    Some(next_rib)
                }
                None => None,
            };
            let mut variants = vec![];
            for (index, name) in names.iter().enumerate() {
                next_rib =
                    cx.map_ast_with_parent(AstNode::EnumVariant(name, node_id, index), next_rib);
                variants.push((Spanned::new(name.name.name, name.name.span), next_rib));
            }
            hir::TypeKind::Enum(variants, ty)
        }
        _ => {
            error!("{:#?}", ty);
            return cx.unimp_msg("lowering of", ty);
        }
    };
    for dim in ty.dims.iter().rev() {
        match *dim {
            ast::TypeDim::Range(ref lhs, ref rhs) => {
                kind = hir::TypeKind::PackedArray(
                    Box::new(kind),
                    cx.map_ast_with_parent(AstNode::Expr(lhs), node_id),
                    cx.map_ast_with_parent(AstNode::Expr(rhs), node_id),
                );
            }
            _ => {
                cx.emit(
                    DiagBuilder2::error(format!(
                        "{} is not a valid packed dimension",
                        dim.desc_full()
                    ))
                    .span(ty.human_span())
                    .add_note("packed array dimensions can only be given as range, e.g. `[31:0]`"),
                );
                return Err(());
            }
        }
    }
    let hir = hir::Type {
        id: node_id,
        span: ty.span,
        kind: kind,
    };
    Ok(HirNode::Type(cx.arena().alloc_hir(hir)))
}

fn lower_expr<'gcx>(
    cx: &impl Context<'gcx>,
    node_id: NodeId,
    expr: &'gcx ast::Expr,
) -> Result<HirNode<'gcx>> {
    use crate::syntax::token::{Lit, Op};
    let kind = match expr.data {
        ast::LiteralExpr(Lit::Number(v, None)) => match v.as_str().parse() {
            Ok(v) => hir::ExprKind::IntConst {
                width: 32,
                value: v,
                signed: true,
                special_bits: BitVec::from_elem(32, false),
                x_bits: BitVec::from_elem(32, false),
            },
            Err(e) => {
                cx.emit(
                    DiagBuilder2::error(format!("`{}` is not a valid integer literal", v))
                        .span(expr.span)
                        .add_note(format!("{}", e)),
                );
                return Err(());
            }
        },
        ast::LiteralExpr(Lit::UnbasedUnsized(c)) => hir::ExprKind::UnsizedConst(c),

        ast::LiteralExpr(Lit::BasedInteger(maybe_size, signed, base, value)) => {
            let value_str = value.as_str();

            // Parse the number's value.
            let parsed = BigInt::parse_bytes(
                value_str
                    .chars()
                    .map(|c| match c {
                        'x' | 'X' | 'z' | 'Z' | '?' => '0',
                        c => c,
                    })
                    .collect::<String>()
                    .as_bytes(),
                match base {
                    'h' => 16,
                    'd' => 10,
                    'o' => 8,
                    'b' => 2,
                    _ => {
                        cx.emit(
                            DiagBuilder2::error(format!("`{}` is not a valid integer base", base))
                                .span(expr.span)
                                .add_note("valid bases are `b`, `o`, `d`, and `h`"),
                        );
                        return Err(());
                    }
                },
            );
            let parsed = match parsed {
                Some(parsed) => parsed,
                None => {
                    cx.emit(
                        DiagBuilder2::error(format!("`{}` is not a valid integer literal", value))
                            .span(expr.span),
                    );
                    return Err(());
                }
            };

            // Parse the size and verify the number fits.
            let size_needed = parsed.bits();
            let size = match maybe_size {
                Some(size) => match size.as_str().parse() {
                    Ok(s) => s,
                    Err(e) => {
                        cx.emit(
                            DiagBuilder2::error(format!("`{}` is not a valid integer size", size))
                                .span(expr.span)
                                .add_note(format!("{}", e)),
                        );
                        return Err(());
                    }
                },
                None => size_needed,
            };
            if size_needed > size {
                cx.emit(DiagBuilder2::warning(format!(
                    "`{}` is too large",
                    value,
                )).span(expr.span).add_note(format!("constant is {} bits wide, but the value `{}{}` needs {} bits to not be truncated", size, base, value, size_needed)));
            }

            // Identify the special bits (x and z) in the input.
            // TODO(fschuiki): Decimal literals are not handled properly.
            let bit_iter = value_str.chars().flat_map(|c| {
                std::iter::repeat(c).take(match base {
                    'h' => 4,
                    'o' => 3,
                    'b' => 1,
                    _ => 0,
                })
            });
            let special_bits: BitVec = bit_iter
                .clone()
                .map(|c| match c {
                    'x' | 'X' | 'z' | 'Z' | '?' => true,
                    _ => false,
                })
                .collect();
            let x_bits: BitVec = bit_iter
                .clone()
                .map(|c| match c {
                    'x' | 'X' => true,
                    _ => false,
                })
                .collect();

            // Assemble the HIR node.
            hir::ExprKind::IntConst {
                width: size,
                value: parsed,
                signed,
                special_bits,
                x_bits,
            }
        }

        ast::LiteralExpr(Lit::Time(int, frac, unit)) => {
            use syntax::token::TimeUnit;
            let mut value = parse_fixed_point_number(cx, expr.span, int, frac)?;
            let magnitude = match unit {
                TimeUnit::Second => 0,
                TimeUnit::MilliSecond => 1,
                TimeUnit::MicroSecond => 2,
                TimeUnit::NanoSecond => 3,
                TimeUnit::PicoSecond => 4,
                TimeUnit::FemtoSecond => 5,
            };
            for _ in 0..magnitude {
                value = value / num::BigInt::from(1000);
            }
            hir::ExprKind::TimeConst(value)
        }

        ast::LiteralExpr(Lit::Str(value)) => {
            hir::ExprKind::StringConst(Spanned::new(value, expr.span))
        }

        ast::IdentExpr(ident) => hir::ExprKind::Ident(Spanned::new(ident.name, ident.span)),
        ast::UnaryExpr {
            op,
            expr: ref arg,
            postfix,
        } => hir::ExprKind::Unary(
            match op {
                Op::Add if !postfix => hir::UnaryOp::Pos,
                Op::Sub if !postfix => hir::UnaryOp::Neg,
                Op::BitNot if !postfix => hir::UnaryOp::BitNot,
                Op::LogicNot if !postfix => hir::UnaryOp::LogicNot,
                Op::Inc if !postfix => hir::UnaryOp::PreInc,
                Op::Dec if !postfix => hir::UnaryOp::PreDec,
                Op::Inc if postfix => hir::UnaryOp::PostInc,
                Op::Dec if postfix => hir::UnaryOp::PostDec,
                Op::BitAnd if !postfix => hir::UnaryOp::RedAnd,
                Op::BitNand if !postfix => hir::UnaryOp::RedNand,
                Op::BitOr if !postfix => hir::UnaryOp::RedOr,
                Op::BitNor if !postfix => hir::UnaryOp::RedNor,
                Op::BitXor if !postfix => hir::UnaryOp::RedXor,
                Op::BitNxor if !postfix => hir::UnaryOp::RedXnor,
                Op::BitXnor if !postfix => hir::UnaryOp::RedXnor,
                _ => {
                    cx.emit(
                        DiagBuilder2::error(format!(
                            "`{}` is not a valid {} operator",
                            op,
                            match postfix {
                                true => "postfix",
                                false => "prefix",
                            }
                        ))
                        .span(expr.span()),
                    );
                    return Err(());
                }
            },
            cx.map_ast_with_parent(AstNode::Expr(arg), node_id),
        ),
        ast::BinaryExpr {
            op,
            ref lhs,
            ref rhs,
        } => hir::ExprKind::Binary(
            match op {
                Op::Add => hir::BinaryOp::Add,
                Op::Sub => hir::BinaryOp::Sub,
                Op::Mul => hir::BinaryOp::Mul,
                Op::Div => hir::BinaryOp::Div,
                Op::Mod => hir::BinaryOp::Mod,
                Op::Pow => hir::BinaryOp::Pow,
                Op::LogicEq => hir::BinaryOp::Eq,
                Op::LogicNeq => hir::BinaryOp::Neq,
                Op::Lt => hir::BinaryOp::Lt,
                Op::Leq => hir::BinaryOp::Leq,
                Op::Gt => hir::BinaryOp::Gt,
                Op::Geq => hir::BinaryOp::Geq,
                Op::LogicAnd => hir::BinaryOp::LogicAnd,
                Op::LogicOr => hir::BinaryOp::LogicOr,
                Op::BitAnd => hir::BinaryOp::BitAnd,
                Op::BitNand => hir::BinaryOp::BitNand,
                Op::BitOr => hir::BinaryOp::BitOr,
                Op::BitNor => hir::BinaryOp::BitNor,
                Op::BitXor => hir::BinaryOp::BitXor,
                Op::BitXnor => hir::BinaryOp::BitXnor,
                Op::BitNxor => hir::BinaryOp::BitXnor,
                Op::LogicShL => hir::BinaryOp::LogicShL,
                Op::LogicShR => hir::BinaryOp::LogicShR,
                Op::ArithShL => hir::BinaryOp::ArithShL,
                Op::ArithShR => hir::BinaryOp::ArithShR,
                _ => {
                    cx.emit(
                        DiagBuilder2::error(format!("`{}` is not a valid binary operator", op,))
                            .span(expr.span()),
                    );
                    return Err(());
                }
            },
            cx.map_ast_with_parent(AstNode::Expr(lhs), node_id),
            cx.map_ast_with_parent(AstNode::Expr(rhs), node_id),
        ),
        ast::MemberExpr { ref expr, name } => hir::ExprKind::Field(
            cx.map_ast_with_parent(AstNode::Expr(expr), node_id),
            Spanned::new(name.name, name.span),
        ),
        ast::IndexExpr {
            ref indexee,
            ref index,
        } => {
            let indexee = cx.map_ast_with_parent(AstNode::Expr(indexee), node_id);
            let mode = match index.data {
                ast::RangeExpr {
                    mode,
                    ref lhs,
                    ref rhs,
                } => hir::IndexMode::Many(
                    mode,
                    cx.map_ast_with_parent(AstNode::Expr(lhs), node_id),
                    cx.map_ast_with_parent(AstNode::Expr(rhs), node_id),
                ),
                _ => hir::IndexMode::One(cx.map_ast_with_parent(AstNode::Expr(index), node_id)),
            };
            hir::ExprKind::Index(indexee, mode)
        }
        ast::CallExpr(ref callee, ref args) => match callee.data {
            ast::SysIdentExpr(ident) => {
                let map_unary = || {
                    Ok(match args.as_slice() {
                        [ast::CallArg {
                            expr: Some(ref arg),
                            ..
                        }] => cx.map_ast_with_parent(AstNode::Expr(arg), node_id),
                        _ => {
                            cx.emit(
                                DiagBuilder2::error(format!("`{}` takes one argument", ident.name))
                                    .span(expr.human_span()),
                            );
                            return Err(());
                        }
                    })
                };
                hir::ExprKind::Builtin(match &*ident.name.as_str() {
                    "clog2" => hir::BuiltinCall::Clog2(map_unary()?),
                    "bits" => hir::BuiltinCall::Bits(map_unary()?),
                    "signed" => hir::BuiltinCall::Signed(map_unary()?),
                    "unsigned" => hir::BuiltinCall::Unsigned(map_unary()?),
                    _ => {
                        cx.emit(
                            DiagBuilder2::warning(format!(
                                "`${}` not supported; ignored",
                                ident.name
                            ))
                            .span(expr.human_span()),
                        );
                        hir::BuiltinCall::Unsupported
                    }
                })
            }
            _ => {
                error!("{:#?}", callee);
                return cx.unimp_msg("lowering of call to", callee.as_ref());
            }
        },
        ast::TernaryExpr {
            ref cond,
            ref true_expr,
            ref false_expr,
        } => hir::ExprKind::Ternary(
            cx.map_ast_with_parent(AstNode::Expr(cond), node_id),
            cx.map_ast_with_parent(AstNode::Expr(true_expr), node_id),
            cx.map_ast_with_parent(AstNode::Expr(false_expr), node_id),
        ),
        ast::ScopeExpr(ref expr, name) => hir::ExprKind::Scope(
            cx.map_ast_with_parent(AstNode::Expr(expr.as_ref()), node_id),
            Spanned::new(name.name, name.span),
        ),
        ast::PatternExpr(ref fields) if fields.is_empty() => hir::ExprKind::EmptyPattern,
        ast::PatternExpr(ref fields) => {
            let deciding_span = fields[0].span;
            match fields[0].data {
                ast::PatternFieldData::Expr(_) => {
                    let mut mapping = vec![];
                    for field in fields {
                        mapping.push(match field.data {
                            ast::PatternFieldData::Expr(ref expr) => {
                                cx.map_ast_with_parent(AstNode::Expr(expr.as_ref()), node_id)
                            }
                            _ => {
                                cx.emit(
                                    DiagBuilder2::error(format!(
                                        "`{}` not a positional pattern",
                                        field.span.extract()
                                    ))
                                    .span(field.span)
                                    .add_note(
                                        "required because first field was a positional pattern,
                                             and all fields must be the same:",
                                    )
                                    .span(deciding_span),
                                );
                                continue;
                            }
                        });
                    }
                    hir::ExprKind::PositionalPattern(mapping)
                }
                ast::PatternFieldData::Repeat(ref count, ref exprs) => {
                    for field in &fields[1..] {
                        cx.emit(
                            DiagBuilder2::error(format!(
                                "`{}` after repeat pattern",
                                field.span.extract()
                            ))
                            .span(field.span)
                            .add_note("repeat patterns must have the form `'{<expr>{...}}`"),
                        );
                    }
                    hir::ExprKind::RepeatPattern(
                        cx.map_ast_with_parent(AstNode::Expr(count), node_id),
                        exprs
                            .iter()
                            .map(|expr| cx.map_ast_with_parent(AstNode::Expr(expr), node_id))
                            .collect(),
                    )
                }
                ast::PatternFieldData::Type(..)
                | ast::PatternFieldData::Member(..)
                | ast::PatternFieldData::Default(..) => {
                    let mut mapping = vec![];
                    for field in fields {
                        mapping.push(match field.data {
                            ast::PatternFieldData::Type(ref ty, ref expr) => (
                                hir::PatternMapping::Type(
                                    cx.map_ast_with_parent(AstNode::Type(ty), node_id),
                                ),
                                cx.map_ast_with_parent(AstNode::Expr(expr.as_ref()), node_id),
                            ),
                            ast::PatternFieldData::Member(ref member, ref expr) => (
                                hir::PatternMapping::Member(
                                    cx.map_ast_with_parent(AstNode::Expr(member.as_ref()), node_id),
                                ),
                                cx.map_ast_with_parent(AstNode::Expr(expr.as_ref()), node_id),
                            ),
                            ast::PatternFieldData::Default(ref expr) => (
                                hir::PatternMapping::Default,
                                cx.map_ast_with_parent(AstNode::Expr(expr.as_ref()), node_id),
                            ),
                            _ => {
                                cx.emit(
                                    DiagBuilder2::error(format!(
                                        "`{}` not a named pattern",
                                        field.span.extract()
                                    ))
                                    .span(field.span)
                                    .add_note(
                                        "required because first field was a named pattern,
                                             and all fields must be the same:",
                                    )
                                    .span(deciding_span),
                                );
                                continue;
                            }
                        });
                    }
                    hir::ExprKind::NamedPattern(mapping)
                }
            }
        }
        ast::ConcatExpr {
            ref repeat,
            ref exprs,
        } => hir::ExprKind::Concat(
            repeat
                .as_ref()
                .map(|expr| cx.map_ast_with_parent(AstNode::Expr(expr), node_id)),
            exprs
                .iter()
                .map(|expr| cx.map_ast_with_parent(AstNode::Expr(expr), node_id))
                .collect(),
        ),
        ast::CastExpr(ref ty, ref expr) => hir::ExprKind::Cast(
            cx.map_ast_with_parent(AstNode::Type(ty), node_id),
            cx.map_ast_with_parent(AstNode::Expr(expr), node_id),
        ),
        ast::InsideExpr(ref expr, ref ranges) => hir::ExprKind::Inside(
            cx.map_ast_with_parent(AstNode::Expr(expr), node_id),
            ranges
                .iter()
                .map(|vr| match vr {
                    ast::ValueRange::Single(expr) => Spanned::new(
                        hir::InsideRange::Single(
                            cx.map_ast_with_parent(AstNode::Expr(expr), node_id),
                        ),
                        expr.span,
                    ),
                    ast::ValueRange::Range { lo, hi, span } => Spanned::new(
                        hir::InsideRange::Range(
                            cx.map_ast_with_parent(AstNode::Expr(lo), node_id),
                            cx.map_ast_with_parent(AstNode::Expr(hi), node_id),
                        ),
                        *span,
                    ),
                })
                .collect(),
        ),
        _ => {
            error!("{:#?}", expr);
            return cx.unimp_msg("lowering of", expr);
        }
    };
    let hir = hir::Expr {
        id: node_id,
        span: expr.span,
        kind: kind,
    };
    Ok(HirNode::Expr(cx.arena().alloc_hir(hir)))
}

/// Parse a fixed point number into a [`BigRational`].
///
/// The fractional part of the number is optional, such that this function may
/// also be used to parse integers into a ratio.
fn parse_fixed_point_number<'gcx>(
    cx: &impl Context<'gcx>,
    span: Span,
    int: Name,
    frac: Option<Name>,
) -> Result<num::BigRational> {
    let mut num_digits = int.to_string();
    let mut denom_digits = String::from("1");
    if let Some(frac) = frac {
        let s = frac.to_string();
        num_digits.push_str(&s);
        denom_digits.extend(s.chars().map(|_| '0'));
    }
    match (num_digits.parse(), denom_digits.parse()) {
        (Ok(a), Ok(b)) => Ok((a, b).into()),
        (Err(e), _) | (_, Err(e)) => {
            let value = match frac {
                Some(frac) => format!("{}.{}", int, frac),
                None => format!("{}", int),
            };
            cx.emit(
                DiagBuilder2::error(format!("`{}` is not a number literal", value))
                    .span(span)
                    .add_note(format!("{}", e)),
            );
            Err(())
        }
    }
}

fn lower_event_expr<'gcx>(
    cx: &impl Context<'gcx>,
    expr: &'gcx ast::EventExpr,
    parent_id: NodeId,
    into: &mut Vec<hir::Event>,
    cond_stack: &mut Vec<NodeId>,
) -> Result<()> {
    match *expr {
        ast::EventExpr::Edge {
            span,
            edge,
            ref value,
        } => {
            into.push(hir::Event {
                span,
                edge,
                expr: cx.map_ast_with_parent(AstNode::Expr(value), parent_id),
                iff: cond_stack.clone(),
            });
        }
        ast::EventExpr::Iff {
            ref expr, ref cond, ..
        } => {
            cond_stack.push(cx.map_ast_with_parent(AstNode::Expr(cond), parent_id));
            lower_event_expr(cx, expr, parent_id, into, cond_stack)?;
            cond_stack.pop().unwrap();
        }
        ast::EventExpr::Or {
            ref lhs, ref rhs, ..
        } => {
            lower_event_expr(cx, lhs, parent_id, into, cond_stack)?;
            lower_event_expr(cx, rhs, parent_id, into, cond_stack)?;
        }
    };
    Ok(())
}

/// Lower a list of genvar declarations.
fn alloc_genvar_init<'gcx>(
    cx: &impl Context<'gcx>,
    stmt: &'gcx ast::Stmt,
    mut parent_id: NodeId,
) -> Result<Vec<NodeId>> {
    let mut ids = vec![];
    match stmt.data {
        ast::GenvarDeclStmt(ref decls) => {
            for decl in decls {
                let id = cx.map_ast_with_parent(AstNode::GenvarDecl(decl), parent_id);
                ids.push(id);
                parent_id = id;
            }
        }
        _ => {
            cx.emit(
                DiagBuilder2::error(format!(
                    "{} is not a valid genvar initialization",
                    stmt.desc_full()
                ))
                .span(stmt.human_span()),
            );
            return Err(());
        }
    }
    Ok(ids)
}

/// Allocate node IDs for a parameter declaration.
fn alloc_param_decl<'gcx>(
    cx: &impl Context<'gcx>,
    param: &'gcx ast::ParamDecl,
    mut next_rib: NodeId,
    into: &mut Vec<NodeId>,
) -> NodeId {
    match param.kind {
        ast::ParamKind::Type(ref decls) => {
            for decl in decls {
                let id = cx.map_ast(AstNode::TypeParam(param, decl));
                cx.set_parent(id, next_rib);
                next_rib = id;
                into.push(id);
            }
        }
        ast::ParamKind::Value(ref decls) => {
            for decl in decls {
                let id = cx.map_ast(AstNode::ValueParam(param, decl));
                cx.set_parent(id, next_rib);
                next_rib = id;
                into.push(id);
            }
        }
    }
    next_rib
}

/// Allocate node IDs for a variable declaration.
fn alloc_var_decl<'gcx>(
    cx: &impl Context<'gcx>,
    decl: &'gcx ast::VarDecl,
    mut next_rib: NodeId,
    into: &mut Vec<NodeId>,
) -> NodeId {
    let type_id = cx.map_ast_with_parent(AstNode::Type(&decl.ty), next_rib);
    next_rib = type_id;
    for name in &decl.names {
        let decl_id = cx.map_ast_with_parent(AstNode::VarDecl(name, decl, type_id), next_rib);
        next_rib = decl_id;
        into.push(decl_id);
    }
    next_rib
}

/// Allocate node IDs for a net declaration.
fn alloc_net_decl<'gcx>(
    cx: &impl Context<'gcx>,
    decl: &'gcx ast::NetDecl,
    mut next_rib: NodeId,
    into: &mut Vec<NodeId>,
) -> NodeId {
    let type_id = cx.map_ast_with_parent(AstNode::Type(&decl.ty), next_rib);
    next_rib = type_id;
    for name in &decl.names {
        let decl_id = cx.map_ast_with_parent(AstNode::NetDecl(name, decl, type_id), next_rib);
        next_rib = decl_id;
        into.push(decl_id);
    }
    next_rib
}

/// Allocate node IDs for a struct member.
fn alloc_struct_member<'gcx>(
    cx: &impl Context<'gcx>,
    member: &'gcx ast::StructMember,
    mut next_rib: NodeId,
    into: &mut Vec<NodeId>,
) -> NodeId {
    let type_id = cx.map_ast_with_parent(AstNode::Type(&member.ty), next_rib);
    next_rib = type_id;
    for name in &member.names {
        let member_id =
            cx.map_ast_with_parent(AstNode::StructMember(name, member, type_id), next_rib);
        next_rib = member_id;
        into.push(member_id);
    }
    next_rib
}

/// Lower a package to HIR.
///
/// This allocates node IDs to everything in the package and registers AST nodes
/// for each ID.
fn lower_package<'gcx>(
    cx: &impl Context<'gcx>,
    node_id: NodeId,
    ast: &'gcx ast::PackageDecl,
) -> Result<HirNode<'gcx>> {
    let mut next_rib = node_id;
    let mut names = Vec::new();
    let mut decls = Vec::new();
    let mut params = Vec::new();
    for item in &ast.items {
        match *item {
            ast::HierarchyItem::VarDecl(ref decl) => {
                next_rib = alloc_var_decl(cx, decl, next_rib, &mut decls);
            }
            ast::HierarchyItem::ParamDecl(ref param) => {
                next_rib = alloc_param_decl(cx, param, next_rib, &mut params);
            }
            ast::HierarchyItem::Typedef(ref def) => {
                next_rib = cx.map_ast_with_parent(AstNode::Typedef(def), next_rib);
                names.push((Spanned::new(def.name.name, def.name.span), next_rib));
            }
            ast::HierarchyItem::SubroutineDecl(ref decl) => warn!(
                "ignoring unsupported subroutine `{}`",
                decl.prototype.name.name
            ),
            _ => {
                cx.emit(
                    DiagBuilder2::error(format!("{} cannot appear in a package", item.desc_full()))
                        .span(item.human_span()),
                );
                return Err(());
            }
        }
    }

    let hir = hir::Package {
        id: node_id,
        name: Spanned::new(ast.name, ast.name_span),
        span: ast.span,
        names,
        decls,
        params,
        last_rib: next_rib,
    };
    Ok(HirNode::Package(cx.arena().alloc_hir(hir)))
}
