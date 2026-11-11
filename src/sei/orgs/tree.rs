use std::borrow::Cow;
use std::cell::{Cell, RefCell};
use std::collections::HashSet;

use html5ever::tendril::StrTendril;
use html5ever::tree_builder::{ElementFlags, NodeOrText, QuirksMode, TreeSink};
use html5ever::{Attribute, QualName};
use scraper::{Html, HtmlTreeSink, Node};

const HTML_NAMESPACE: &str = "http://www.w3.org/1999/xhtml";
const FOREIGN_NAMESPACES: [&str; 2] = [
    "http://www.w3.org/2000/svg",
    "http://www.w3.org/1998/Math/MathML",
];

type Handle = <HtmlTreeSink as TreeSink>::Handle;

pub(super) struct GoTreeSink {
    inner: HtmlTreeSink,
    ignoring: Cell<bool>,
    annotation_points: RefCell<HashSet<Handle>>,
}

impl GoTreeSink {
    pub(super) fn new() -> GoTreeSink {
        GoTreeSink {
            inner: HtmlTreeSink::new(Html::new_document()),
            ignoring: Cell::new(false),
            annotation_points: RefCell::new(HashSet::new()),
        }
    }

    fn stops_at(&self, place: &Handle, child: &NodeOrText<Handle>) -> bool {
        if self.ignoring.get() {
            return true;
        }
        let NodeOrText::AppendNode(node) = child else {
            return false;
        };
        if !(self.is_html_template(node) && self.within_foreign_content(place)) {
            return false;
        }
        self.ignoring.set(true);
        true
    }

    fn is_html_template(&self, handle: &Handle) -> bool {
        let html = self.inner.0.borrow();
        html.tree.get(*handle).is_some_and(|node| {
            matches!(node.value(), Node::Element(element)
                if &*element.name.ns == HTML_NAMESPACE && &*element.name.local == "template")
        })
    }

    fn within_foreign_content(&self, handle: &Handle) -> bool {
        let html = self.inner.0.borrow();
        let Some(node) = html.tree.get(*handle) else {
            return false;
        };
        std::iter::once(node)
            .chain(node.ancestors())
            .any(|node| match node.value() {
                Node::Element(element) => FOREIGN_NAMESPACES.contains(&&*element.name.ns),
                _ => false,
            })
    }

    fn has_parent(&self, handle: &Handle) -> bool {
        let html = self.inner.0.borrow();
        html.tree
            .get(*handle)
            .is_some_and(|node| node.parent().is_some())
    }
}

impl TreeSink for GoTreeSink {
    type Handle = Handle;
    type Output = Html;
    type ElemName<'a> = <HtmlTreeSink as TreeSink>::ElemName<'a>;

    fn finish(self) -> Html {
        self.inner.finish()
    }

    fn parse_error(&self, msg: Cow<'static, str>) {
        self.inner.parse_error(msg);
    }

    fn get_document(&self) -> Handle {
        self.inner.get_document()
    }

    fn elem_name<'a>(&'a self, target: &'a Handle) -> Self::ElemName<'a> {
        self.inner.elem_name(target)
    }

    fn create_element(&self, name: QualName, attrs: Vec<Attribute>, flags: ElementFlags) -> Handle {
        let annotation_point = flags.mathml_annotation_xml_integration_point;
        let handle = self.inner.create_element(name, attrs, flags);
        if annotation_point {
            self.annotation_points.borrow_mut().insert(handle);
        }
        handle
    }

    fn create_comment(&self, text: StrTendril) -> Handle {
        self.inner.create_comment(text)
    }

    fn create_pi(&self, target: StrTendril, data: StrTendril) -> Handle {
        self.inner.create_pi(target, data)
    }

    fn append(&self, parent: &Handle, child: NodeOrText<Handle>) {
        if self.stops_at(parent, &child) {
            return;
        }
        self.inner.append(parent, child);
    }

    fn append_based_on_parent_node(
        &self,
        element: &Handle,
        prev_element: &Handle,
        child: NodeOrText<Handle>,
    ) {
        if self.has_parent(element) {
            self.append_before_sibling(element, child);
            return;
        }
        self.append(prev_element, child);
    }

    fn append_doctype_to_document(
        &self,
        name: StrTendril,
        public_id: StrTendril,
        system_id: StrTendril,
    ) {
        if self.ignoring.get() {
            return;
        }
        self.inner
            .append_doctype_to_document(name, public_id, system_id);
    }

    fn get_template_contents(&self, target: &Handle) -> Handle {
        self.inner.get_template_contents(target)
    }

    fn same_node(&self, x: &Handle, y: &Handle) -> bool {
        self.inner.same_node(x, y)
    }

    fn set_quirks_mode(&self, mode: QuirksMode) {
        self.inner.set_quirks_mode(mode);
    }

    fn append_before_sibling(&self, sibling: &Handle, new_node: NodeOrText<Handle>) {
        if self.stops_at(sibling, &new_node) {
            return;
        }
        self.inner.append_before_sibling(sibling, new_node);
    }

    fn add_attrs_if_missing(&self, target: &Handle, attrs: Vec<Attribute>) {
        if self.ignoring.get() {
            return;
        }
        self.inner.add_attrs_if_missing(target, attrs);
    }

    fn remove_from_parent(&self, target: &Handle) {
        if self.ignoring.get() {
            return;
        }
        self.inner.remove_from_parent(target);
    }

    fn reparent_children(&self, node: &Handle, new_parent: &Handle) {
        if self.ignoring.get() {
            return;
        }
        self.inner.reparent_children(node, new_parent);
    }

    fn is_mathml_annotation_xml_integration_point(&self, handle: &Handle) -> bool {
        self.annotation_points.borrow().contains(handle)
    }

    fn allow_declarative_shadow_roots(&self, _intended_parent: &Handle) -> bool {
        false
    }
}
