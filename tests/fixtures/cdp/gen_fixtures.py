"""Generate CDP Accessibility.getFullAXTree fixtures that mimic real Chrome output.

Usage: python gen_fixtures.py <out_dir>
"""
import json
import os
import sys

FRAME = "8A3C9E1F4B2D6E7A9C0B1D2E3F4A5B6C"
INTERNAL = {"RootWebArea", "StaticText", "InlineTextBox", "LabelText", "LineBreak",
            "DisclosureTriangle", "MenuListPopup"}
CHROME_ROLE = {"RootWebArea": 144, "StaticText": 158, "InlineTextBox": 101, "generic": 88,
               "heading": 96, "paragraph": 133, "link": 110, "button": 9, "textbox": 170,
               "checkbox": 14, "list": 111, "listitem": 115, "main": 118, "banner": 7,
               "contentinfo": 23, "navigation": 128, "dialog": 32, "LabelText": 104,
               "none": 0, "form": 85, "image": 98, "alert": 2, "searchbox": 149,
               "search": 148, "complementary": 21, "article": 4, "region": 140,
               "tablist": 165, "tab": 163, "tabpanel": 166, "combobox": 18,
               "MenuListPopup": 122, "option": 131, "DisclosureTriangle": 36, "group": 94,
               "switch": 161, "radiogroup": 138, "radio": 137}


def p(name, typ, value):
    return {"name": name, "value": {"type": typ, "value": value}}


class Tree:
    def __init__(self, title, url):
        self.nodes = []
        self.next_node = 0
        self.next_backend = 1
        self.root = self.add("RootWebArea", title, None,
                             [p("focusable", "booleanOrUndefined", True),
                              p("url", "string", url)])

    def add(self, role, name=None, parent=None, props=None, value=None, ignored=False,
            backend=True, desc=None, bid=None):
        self.next_node += 1
        node = {"nodeId": str(self.next_node), "ignored": ignored}
        if ignored:
            node["ignoredReasons"] = [{"name": "uninteresting",
                                       "value": {"type": "boolean", "value": True}}]
            node["role"] = {"type": "role", "value": role or "none"}
        else:
            node["role"] = {"type": "internalRole" if role in INTERNAL else "role",
                            "value": role}
        node["chromeRole"] = {"type": "internalRole", "value": CHROME_ROLE.get(role, 0)}
        if not ignored:
            node["name"] = {"type": "computedString", "value": name or ""}
            if name and role not in ("StaticText", "InlineTextBox"):
                node["name"]["sources"] = [{"type": "contents", "value": {"type": "computedString", "value": name}}]
            if desc is not None:
                node["description"] = {"type": "computedString", "value": desc}
            if value is not None:
                node["value"] = {"type": "string", "value": value}
            node["properties"] = props or []
        node["childIds"] = []
        if parent is not None:
            node["parentId"] = parent["nodeId"]
            parent["childIds"].append(node["nodeId"])
        if bid is not None:
            node["backendDOMNodeId"] = bid
        elif backend:
            node["backendDOMNodeId"] = self.next_backend
            self.next_backend += 1
        node["frameId"] = FRAME
        self.nodes.append(node)
        return node

    # --- helpers -------------------------------------------------------
    def text(self, parent, s, bid=None):
        t = self.add("StaticText", s, parent, bid=bid)
        self.add("InlineTextBox", s, t, backend=False)
        return t

    def ignored(self, parent):
        return self.add("none", None, parent, ignored=True)

    def generic(self, parent):
        return self.add("generic", None, parent)

    def heading(self, parent, s, level):
        h = self.add("heading", s, parent, [p("level", "integer", level)])
        self.text(h, s)
        return h

    def para(self, parent, s):
        para = self.add("paragraph", None, parent)
        self.text(para, s)
        return para

    def link(self, parent, s, url):
        a = self.add("link", s, parent, [p("focusable", "booleanOrUndefined", True),
                                         p("url", "string", url)])
        self.text(a, s)
        return a

    def button(self, parent, s, disabled=False, extra=None):
        props = [p("focusable", "booleanOrUndefined", True)]
        if disabled:
            props = [p("disabled", "boolean", True)]
        props += extra or []
        b = self.add("button", s, parent, props)
        self.text(b, s)
        return b

    def textbox(self, parent, name, value="", required=False, focused=False, invalid=False,
                multiline=False, role="textbox"):
        props = [p("invalid", "token", "true" if invalid else "false"),
                 p("focusable", "booleanOrUndefined", True)]
        if focused:
            props.append(p("focused", "booleanOrUndefined", True))
        props += [p("editable", "token", "plaintext"),
                  p("settable", "booleanOrUndefined", True),
                  p("multiline", "boolean", multiline),
                  p("readonly", "boolean", False),
                  p("required", "boolean", required)]
        return self.add(role, name, parent, props, value=value)

    def label(self, parent, s):
        lab = self.add("LabelText", None, parent)
        self.text(lab, s)
        return lab

    def checkbox(self, parent, name, checked, role="checkbox"):
        return self.add(role, name, parent, [p("invalid", "token", "false"),
                                             p("focusable", "booleanOrUndefined", True),
                                             p("checked", "tristate", checked)])

    def dump(self, path):
        with open(path, "w", encoding="utf-8", newline="\n") as f:
            json.dump({"nodes": self.nodes}, f, ensure_ascii=False, indent=1)
            f.write("\n")


def example_com():
    t = Tree("Example Domain", "https://example.com/")
    body = t.ignored(t.root)
    div = t.ignored(body)
    t.heading(div, "Example Domain", 1)
    t.para(div, "This domain is for use in illustrative examples in documents. You may use this "
                "domain in literature without prior coordination or asking for permission.")
    pa = t.add("paragraph", None, div)
    t.link(pa, "More information...", "https://www.iana.org/domains/example")
    return t


def login(after=False):
    t = Tree("Sign in – Acme", "https://acme.com/login")
    body = t.ignored(t.root)
    banner = t.add("banner", None, body)
    t.link(banner, "Home", "https://acme.com/")
    main = t.add("main", None, body)
    t.heading(main, "Sign in", 1)
    t.para(main, "Welcome back! Please sign in to continue.")
    if after:
        alert = t.add("alert", None, main, [p("live", "token", "assertive"),
                                             p("atomic", "boolean", True),
                                             p("relevant", "tokenList", "additions text")])
        t.text(alert, "Wrong password", bid=500)
    else:
        # An empty live region: common on real pages, must not be printed.
        t.add("alert", None, main, [p("live", "token", "assertive")])
    form = t.add("form", None, main)
    t.label(form, "Email")
    t.textbox(form, "Email", value="jane@acme.com" if after else "", required=True,
              focused=not after)
    t.label(form, "Password")
    # Chrome masks password values with U+2022 bullets.
    t.textbox(form, "Password", value="•" * 8, invalid=after, focused=after)
    row = t.generic(form)
    t.checkbox(row, "Remember me", "false")
    t.label(row, "Remember me")
    t.button(form, "Sign in", disabled=not after)
    providers = t.add("list", "Providers", main)
    for prov in ["Google", "GitHub", "Microsoft", "Apple", "Facebook", "X", "LinkedIn",
                 "GitLab", "Bitbucket", "Slack", "Discord", "Okta"]:
        li = t.add("listitem", None, providers, [p("level", "integer", 1)])
        t.button(li, prov)
    footer = t.add("contentinfo", None, body)
    t.text(footer, "© 2026 Acme Inc.")
    t.link(footer, "Privacy", "https://acme.com/privacy")
    t.link(footer, "Terms", "https://acme.com/terms")
    if not after:
        dlg = t.add("dialog", "Cookies", body, [p("focusable", "booleanOrUndefined", True),
                                                 p("modal", "boolean", True)])
        t.para(dlg, "We use cookies to keep you signed in.")
        t.button(dlg, "Accept all")
        t.button(dlg, "Reject all")
    return t


def products():
    t = Tree("Shop – Acme", "https://shop.acme.com/products")
    body = t.ignored(t.root)
    banner = t.add("banner", None, body)
    t.link(banner, "Acme Shop", "https://shop.acme.com/")
    search = t.add("search", None, banner)
    t.textbox(search, "Search products", role="searchbox")
    t.link(banner, "Cart (0)", "https://shop.acme.com/cart")
    nav = t.add("navigation", "Categories", body)
    cats = t.add("list", None, nav)
    for c in ["Mice", "Keyboards", "Monitors", "Headsets", "Cables", "Sale"]:
        li = t.add("listitem", None, cats, [p("level", "integer", 1)])
        t.link(li, c, "https://shop.acme.com/c/" + c.lower())
    main = t.add("main", None, body)
    t.heading(main, "All products", 1)
    t.para(main, "Showing 40 of 312 products")
    lst = t.add("list", "Products", main)
    for i in range(1, 41):
        li = t.add("listitem", None, lst, [p("level", "integer", 1)])
        card = t.generic(li)
        img = t.add("image", "Product %d photo" % i, card)
        t.link(card, "Product %d" % i, "https://shop.acme.com/p/%d" % i)
        t.text(card, "$%d.99" % (10 + i))
        t.button(card, "Add to cart")
        del img
    pag = t.add("navigation", "Pagination", main)
    for s in ["1", "2", "3", "Next"]:
        t.link(pag, s, "https://shop.acme.com/products?page=" + s)
    footer = t.add("contentinfo", None, body)
    t.text(footer, "© 2026 Acme Shop")
    return t


def nested():
    t = Tree("Understanding Accessibility Trees – Acme Blog", "https://blog.acme.com/ax-trees")
    body = t.ignored(t.root)
    banner = t.add("banner", None, body)
    t.link(banner, "Acme Blog", "https://blog.acme.com/")
    nav = t.add("navigation", "Primary", banner)
    ul = t.add("list", None, nav)
    for s in ["Home", "Articles", "Tutorials", "About", "Contact"]:
        li = t.add("listitem", None, ul, [p("level", "integer", 1)])
        t.link(li, s, "https://blog.acme.com/" + s.lower())
    search = t.add("search", None, banner)
    t.textbox(search, "Search articles", role="searchbox")
    t.button(search, "Search")

    main = t.add("main", None, body)
    art = t.add("article", None, main)
    t.heading(art, "Understanding Accessibility Trees", 1)
    t.para(art, "Browsers expose a tree of semantic objects to assistive technology.")
    tabs = t.add("tablist", None, art, [p("orientation", "token", "horizontal")])
    for s, sel in [("Overview", True), ("Details", False), ("FAQ", False)]:
        tab = t.add("tab", s, tabs, [p("focusable", "booleanOrUndefined", True),
                                     p("selected", "booleanOrUndefined", sel)])
        t.text(tab, s)
    panel = t.add("tabpanel", "Overview", art)
    t.para(panel, "The accessibility tree is derived from the DOM.")
    details = t.add("group", None, panel)
    summary = t.add("DisclosureTriangle", "Show more", details,
                    [p("focusable", "booleanOrUndefined", True),
                     p("expanded", "booleanOrUndefined", False)])
    t.text(summary, "Show more")

    comments = t.add("region", "Comments", main)
    t.heading(comments, "Comments", 2)
    form = t.add("form", "Add comment", comments)
    t.textbox(form, "Comment", multiline=True)
    combo = t.add("combobox", "Sort by", form,
                  [p("focusable", "booleanOrUndefined", True),
                   p("expanded", "booleanOrUndefined", False),
                   p("hasPopup", "token", "menu")], value="Newest")
    popup = t.add("MenuListPopup", None, combo, backend=False)
    for s, sel in [("Newest", True), ("Oldest", False), ("Top rated", False)]:
        t.add("option", s, popup, [p("focusable", "booleanOrUndefined", True),
                                   p("selected", "booleanOrUndefined", sel)])
    t.button(form, "Post comment")

    aside = t.add("complementary", None, body)
    t.heading(aside, "Related", 2)
    rel = t.add("list", None, aside)
    for s in ["What is ARIA?", "Landmarks 101", "Testing with screen readers"]:
        li = t.add("listitem", None, rel, [p("level", "integer", 1)])
        t.link(li, s, "https://blog.acme.com/r")
    footer = t.add("contentinfo", None, body)
    fnav = t.add("navigation", "Footer", footer)
    for s in ["Privacy", "Terms", "RSS"]:
        t.link(fnav, s, "https://blog.acme.com/" + s.lower())
    return t


def password_page():
    t = Tree("Security settings – Acme", "https://acme.com/settings/security")
    body = t.ignored(t.root)
    main = t.add("main", None, body)
    t.heading(main, "Security", 2)
    form = t.add("form", "Change password", main)
    # Simulates a build that exposes the raw value: we must still never print it.
    t.textbox(form, "Current password", value="hunter2-secret", required=True)
    t.textbox(form, "New password", value="•" * 10, required=True)
    # A password input whose label does not say "password": only DOM info reveals it.
    t.textbox(form, "PIN", value="4321")
    t.checkbox(form, "Enable two-factor authentication", "true")
    t.checkbox(form, "Email alerts", "false", role="switch")
    rg = t.add("radiogroup", "Session timeout", form)
    for s, c in [("15 minutes", "true"), ("1 hour", "false"), ("Never", "false")]:
        t.checkbox(rg, s, c, role="radio")
    t.button(form, "Save changes")
    return t


def main_():
    out = sys.argv[1]
    os.makedirs(out, exist_ok=True)
    example_com().dump(os.path.join(out, "example_com.json"))
    login().dump(os.path.join(out, "login_form.json"))
    login(after=True).dump(os.path.join(out, "login_form_after_submit.json"))
    products().dump(os.path.join(out, "product_list.json"))
    nested().dump(os.path.join(out, "nested_landmarks.json"))
    password_page().dump(os.path.join(out, "password_checkbox.json"))


main_()
