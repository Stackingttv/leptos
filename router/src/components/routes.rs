use crate::{
    matching::{
        expand_optionals, get_route_matches, join_paths, Branch, Matcher,
        RouteDefinition, RouteMatch,
    },
    RouteContext, RouterContext, animation::*
};
use leptos::{leptos_dom::HydrationCtx, *};
use std::{
    cell::{Cell, RefCell},
    cmp::Reverse,
    ops::IndexMut,
    rc::Rc,
};

/// Contains route definitions and manages the actual routing process.
///
/// You should locate the `<Routes/>` component wherever on the page you want the routes to appear.
#[component]
pub fn Routes(
    cx: Scope,
    /// Base path relative at which the routes are mounted.
    #[prop(optional)]
    base: Option<String>,
    children: Children,
) -> impl IntoView {
    let router = use_context::<RouterContext>(cx)
        .expect("<Routes/> component should be nested within a <Router/>.");
    let base_route = router.base();
    let branches = build_branches(cx, children, base);

    #[cfg(feature = "ssr")]
    if let Some(context) = use_context::<crate::PossibleBranchContext>(cx) {
        *context.0.borrow_mut() = branches.clone();
    }

    let next_route = router.pathname();
    let current_route = next_route;

    let root_equal = Rc::new(Cell::new(true));
    let route_states = route_states(cx, &router, branches, current_route, &root_equal);

    let id = HydrationCtx::id();
    let root = root_route(cx, base_route, route_states, root_equal);
    leptos::leptos_dom::DynChild::new_with_id(id, move || root.get())
        .into_view(cx)
}

/// Contains route definitions and manages the actual routing process, with animated transitions 
/// between routes.
///
/// You should locate the `<AnimatedRoutes/>` component wherever on the page you want the routes to appear.
#[component]
pub fn AnimatedRoutes(
    cx: Scope,
    /// Base path relative at which the routes are mounted.
    #[prop(optional)]
    base: Option<String>,
    /// Configuration for router animations.
    #[prop(optional)]
    animation: Animation,
    children: Children,
) -> impl IntoView {
    let router = use_context::<RouterContext>(cx)
        .expect("<Routes/> component should be nested within a <Router/>.");
    let base_route = router.base();
    let branches = build_branches(cx, children, base);

    #[cfg(feature = "ssr")]
    if let Some(context) = use_context::<crate::PossibleBranchContext>(cx) {
        *context.0.borrow_mut() = branches.clone();
    }

    let (animation_state, set_animation_state) =
        create_signal(cx, AnimationState::Finally);
    let next_route = router.pathname();
    let animation_and_route = create_memo(cx, {
        let animation = animation.clone();
        move |prev: Option<&(AnimationState, String)>| {
            leptos::log!("animation_and_route {prev:?}");
            let next_route = next_route.get();
            match prev {
                None => (animation_state.get(), next_route),
                Some((prev_state, prev_route)) => {
                    let (next_state, can_advance) =
                        animation.next_state(prev_state);

                    if can_advance {
                        (next_state, next_route)
                    } else {
                        (next_state, prev_route.to_owned())
                    }
                }
            }
        }
    });
    let current_animation =
        create_memo(cx, move |_| animation_and_route.get().0);
    let current_route = match animation {
        Animation::None => next_route,
        Animation::Classes { .. } => {
            create_memo(cx, move |_| animation_and_route.get().1)
        }
    };

    let root_equal = Rc::new(Cell::new(true));
    let route_states = route_states(cx, &router, branches, current_route, &root_equal);

    let id = HydrationCtx::id();
    let root = root_route(cx, base_route, route_states, root_equal);

    let anim_config = animation.clone();
    match animation {
        Animation::None => {
            leptos::leptos_dom::DynChild::new_with_id(id, move || root.get())
                .into_view(cx)
        }
        Animation::Classes {
            start,
            outro,
            intro,
            finally,
        } => html::div(cx)
            .attr(
                "class",
                (cx, move || match current_animation.get() {
                    AnimationState::Outro => outro.unwrap_or_default(),
                    AnimationState::Start => start.unwrap_or_default(),
                    AnimationState::Intro => intro.unwrap_or_default(),
                    AnimationState::Finally => finally.unwrap_or_default(),
                }),
            )
            .on(leptos::ev::animationend, move |_| {
                let current = current_animation.get();
                set_animation_state.update(|current_state| {
                    let (next, _) = anim_config.next_state(&current);
                    *current_state = next;
                    leptos::log!("animation updating to {next:?}");
                });
            })
            .child(move || root.get())
            .into_view(cx),
    }
}

fn build_branches(cx: Scope, children: Children, base: Option<String>) -> Vec<Branch> {
    let mut branches = Vec::new();
    let children = fragment_to_route_definitions(&children(cx));

    create_branches(
        &children,
        &base.unwrap_or_default(),
        &mut Vec::new(),
        &mut branches,
    );

    branches
}

fn fragment_to_route_definitions(frag: &Fragment) -> Vec<RouteDefinition> {
    frag
        .as_children()
        .iter()
        .filter_map(view_as_route_definition)
        .cloned()
        .collect::<Vec<_>>()
}

fn view_as_route_definition(view: &View) -> Option<&RouteDefinition> {
    let def = view
        .as_transparent()
        .and_then(|t| t.downcast_ref::<RouteDefinition>());
    if def.is_none() {
        warn!(
            "[NOTE] The <Routes/> component should include *only* \
                <Route/>or <ProtectedRoute/> components, or some \
                #[component(transparent)] that returns a RouteDefinition."
        );
    }
    def
}

fn route_states(cx: Scope, router: &RouterContext, branches: Vec<Branch>, current_route: Memo<String>, root_equal: &Rc<Cell<bool>>) -> Memo<RouterState> {
    // whenever path changes, update matches
    let matches = create_memo(cx, move |_| {
        get_route_matches(&branches, current_route.get())
    });

    // iterate over the new matches, reusing old routes when they are the same
    // and replacing them with new routes when they differ
    let next: Rc<RefCell<Vec<RouteContext>>> = Default::default();
    let router = Rc::clone(&router.inner);

    create_memo(cx, {
        let root_equal = Rc::clone(root_equal);
        move |prev: Option<&RouterState>| {
            root_equal.set(true);
            next.borrow_mut().clear();

            let next_matches = matches.get();
            let prev_matches = prev.as_ref().map(|p| &p.matches);
            let prev_routes = prev.as_ref().map(|p| &p.routes);

            // are the new route matches the same as the previous route matches so far?
            let mut equal = prev_matches
                .map(|prev_matches| next_matches.len() == prev_matches.len())
                .unwrap_or(false);

            for i in 0..next_matches.len() {
                let next = next.clone();
                let prev_match = prev_matches.and_then(|p| p.get(i));
                let next_match = next_matches.get(i).unwrap();

                match (prev_routes, prev_match) {
                    (Some(prev), Some(prev_match))
                        if next_match.route.key == prev_match.route.key
                            && next_match.route.id == prev_match.route.id =>
                    {
                        let prev_one = { prev.borrow()[i].clone() };
                        if next_match.path_match.path != prev_one.path() {
                            prev_one
                                .set_path(next_match.path_match.path.clone());
                        }
                        if i >= next.borrow().len() {
                            next.borrow_mut().push(prev_one);
                        } else {
                            *(next.borrow_mut().index_mut(i)) = prev_one;
                        }
                    }
                    _ => {
                        equal = false;
                        if i == 0 {
                            root_equal.set(false);
                        }

                        let next = next.clone();

                        let next = next.clone();
                        let next_ctx = RouteContext::new(
                            cx,
                            &RouterContext { inner: Rc::clone(&router) },
                            {
                                let next = next.clone();
                                move |cx| {
                                    if let Some(route_states) =
                                        use_context::<Memo<RouterState>>(cx)
                                    {
                                        route_states.with(|route_states| {
                                            let routes =
                                                route_states.routes.borrow();
                                            routes.get(i + 1).cloned()
                                        })
                                    } else {
                                        next.borrow().get(i + 1).cloned()
                                    }
                                }
                            },
                            move || matches.with(|m| m.get(i).cloned()),
                        );

                        if let Some(next_ctx) = next_ctx {
                            if next.borrow().len() > i + 1 {
                                next.borrow_mut()[i] = next_ctx;
                            } else {
                                next.borrow_mut().push(next_ctx);
                            }
                        }
                    }
                }
            }

            if let Some(prev) = &prev {
                if equal {
                    RouterState {
                        matches: next_matches.to_vec(),
                        routes: prev_routes.cloned().unwrap_or_default(),
                        root: prev.root.clone(),
                    }
                } else {
                    let root = next.borrow().get(0).cloned();
                    RouterState {
                        matches: next_matches.to_vec(),
                        routes: Rc::new(RefCell::new(next.borrow().to_vec())),
                        root,
                    }
                }
            } else {
                let root = next.borrow().get(0).cloned();
                RouterState {
                    matches: next_matches.to_vec(),
                    routes: Rc::new(RefCell::new(next.borrow().to_vec())),
                    root,
                }
            }
        }
    })
}

fn root_route(cx: Scope, base_route: RouteContext, route_states: Memo<RouterState>, root_equal: Rc<Cell<bool>>) -> Memo<Option<View>> {
    let root_cx = RefCell::new(None);

    create_memo(cx, move |prev| {
        provide_context(cx, route_states);
        route_states.with(|state| {
            if state.routes.borrow().is_empty() {
                Some(base_route.outlet(cx).into_view(cx))
            } else {
                let root = state.routes.borrow();
                let root = root.get(0);
                if let Some(route) = root {
                    provide_context(cx, route.clone());
                }

                if prev.is_none() || !root_equal.get() {
                    let (root_view, _) = cx.run_child_scope(|cx| {
                        let prev_cx = std::mem::replace(
                            &mut *root_cx.borrow_mut(),
                            Some(cx),
                        );
                        if let Some(prev_cx) = prev_cx {
                            prev_cx.dispose();
                        }
                        root.as_ref()
                            .map(|route| route.outlet(cx).into_view(cx))
                    });
                    root_view
                } else {
                    prev.cloned().unwrap()
                }
            }
        })
    })
}

#[derive(Clone, Debug, PartialEq)]
struct RouterState {
    matches: Vec<RouteMatch>,
    routes: Rc<RefCell<Vec<RouteContext>>>,
    root: Option<RouteContext>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RouteData {
    pub id: usize,
    pub key: RouteDefinition,
    pub pattern: String,
    pub original_path: String,
    pub matcher: Matcher,
}

impl RouteData {
    fn score(&self) -> i32 {
        let (pattern, splat) = match self.pattern.split_once("/*") {
            Some((p, s)) => (p, Some(s)),
            None => (self.pattern.as_str(), None),
        };
        let segments = pattern
            .split('/')
            .filter(|n| !n.is_empty())
            .collect::<Vec<_>>();
        #[allow(clippy::bool_to_int_with_if)] // on the splat.is_none()
        segments.iter().fold(
            (segments.len() as i32) - if splat.is_none() { 0 } else { 1 },
            |score, segment| {
                score + if segment.starts_with(':') { 2 } else { 3 }
            },
        )
    }
}

fn create_branches(
    route_defs: &[RouteDefinition],
    base: &str,
    stack: &mut Vec<RouteData>,
    branches: &mut Vec<Branch>,
) {
    for def in route_defs {
        let routes = create_routes(def, base);
        for route in routes {
            stack.push(route.clone());

            if def.children.is_empty() {
                let branch = create_branch(stack, branches.len());
                branches.push(branch);
            } else {
                create_branches(&def.children, &route.pattern, stack, branches);
            }

            stack.pop();
        }
    }

    if stack.is_empty() {
        branches.sort_by_key(|branch| Reverse(branch.score));
    }
}

pub(crate) fn create_branch(routes: &[RouteData], index: usize) -> Branch {
    Branch {
        routes: routes.to_vec(),
        score: routes.last().unwrap().score() * 10000 - (index as i32),
    }
}

fn create_routes(route_def: &RouteDefinition, base: &str) -> Vec<RouteData> {
    let RouteDefinition { children, .. } = route_def;
    let is_leaf = children.is_empty();
    let mut acc = Vec::new();
    for original_path in expand_optionals(&route_def.path) {
        let path = join_paths(base, &original_path);
        let pattern = if is_leaf {
            path
        } else {
            path.split("/*")
                .next()
                .map(|n| n.to_string())
                .unwrap_or(path)
        };
        acc.push(RouteData {
            key: route_def.clone(),
            id: route_def.id,
            matcher: Matcher::new_with_partial(&pattern, !is_leaf),
            pattern,
            original_path: original_path.to_string(),
        });
    }
    acc
}
