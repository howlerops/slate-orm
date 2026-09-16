/**
 * The documentation sidebar and the per-page table of contents.
 *
 * The sidebar is stated **once**, here, and rendered into every page. The
 * alternative — writing the same list into each of six files — is the shape
 * this repository keeps finding bugs in: a thing said N times is a thing that
 * goes stale in N-1 of them, and a nav link that quietly stops matching the
 * page it points at is invisible until somebody follows it.
 *
 * The cost is that the sidebar needs JavaScript. That is paid for honestly:
 * every page carries a `<noscript>` link to this file's own listing as plain
 * markup, the page's *content* never depends on this script, and the header
 * links across the site are ordinary `<a>` elements in the HTML.
 *
 * There is no build step and no framework. This is a module, a list and two
 * loops.
 */

/**
 * The whole documentation tree.
 *
 * Order is reading order: a visitor who starts at the top and works down meets
 * each idea after the ones it depends on. `href` is relative to `site/docs/`,
 * which is where every page lives.
 */
export const SECTIONS = [
  {
    title: "Getting started",
    pages: [
      ["index.html", "Introduction"],
      ["quickstart.html", "Quickstart"],
      ["concepts.html", "Five things to know first"],
    ],
  },
  {
    title: "The database",
    pages: [
      ["features.html", "What it does"],
      ["storage.html", "Keys, rows and indexes"],
      ["security.html", "Security in the kernel"],
    ],
  },
  {
    title: "Running it",
    pages: [
      ["deployment.html", "One writer, many readers"],
      ["clients.html", "The three clients"],
    ],
  },
  {
    title: "Honesty",
    pages: [
      ["limits.html", "What it is not"],
      ["roadmap.html", "Against the other ORMs"],
    ],
  },
];

/** The flat reading order, for the previous/next pair at the foot of a page. */
const ORDER = SECTIONS.flatMap((section) => section.pages);

/** The file this page is, as the sidebar spells it. */
function currentPage() {
  const last = window.location.pathname.split("/").pop();
  // A directory URL (`/docs/`) serves `index.html` and reports an empty last
  // segment, which would match nothing and leave every link unhighlighted.
  return last === "" ? "index.html" : last;
}

function element(tag, className, text) {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
}

function renderSidebar(host, here) {
  for (const section of SECTIONS) {
    host.append(element("h2", "nav-section", section.title));
    const list = element("ul", "nav-list");
    for (const [href, label] of section.pages) {
      const item = document.createElement("li");
      const link = element("a", undefined, label);
      link.href = href;
      if (href === here) {
        link.setAttribute("aria-current", "page");
      }
      item.append(link);
      list.append(item);
    }
    host.append(list);
  }
}

/**
 * The right-hand contents, built from the page's own `<h2>`s.
 *
 * Derived rather than declared, so a heading added to a page cannot be missing
 * from its contents. A heading with no `id` is given one from its text — the
 * alternative is silently dropping it, which would make the list quietly
 * incomplete rather than visibly wrong.
 */
function renderToc(host, article) {
  const headings = [...article.querySelectorAll("h2")];
  if (headings.length < 2) {
    // One heading is not a table of contents, it is a restatement of the title.
    host.closest(".doc-toc")?.remove();
    return;
  }
  host.append(element("h2", "nav-section", "On this page"));
  const list = element("ul", "nav-list");
  for (const heading of headings) {
    if (!heading.id) {
      heading.id = heading.textContent
        .toLowerCase()
        .replace(/[^a-z0-9]+/g, "-")
        .replace(/^-|-$/g, "");
    }
    const item = document.createElement("li");
    const link = element("a", undefined, heading.textContent);
    link.href = `#${heading.id}`;
    item.append(link);
    list.append(item);
  }
  host.append(list);
}

function renderPrevNext(host, here) {
  const at = ORDER.findIndex(([href]) => href === here);
  if (at < 0) return;
  const previous = ORDER[at - 1];
  const next = ORDER[at + 1];
  // A spacer keeps "next" hard right on the first page, where there is no
  // "previous" to push it there.
  host.append(
    previous ? link("prev", previous, "←") : element("span"),
    next ? link("next", next, "→") : element("span"),
  );

  function link(kind, [href, label], arrow) {
    const anchor = element("a", `pager pager-${kind}`);
    anchor.href = href;
    anchor.append(
      element("span", "pager-kind", kind === "prev" ? "Previous" : "Next"),
      element("span", "pager-label", kind === "prev" ? `${arrow} ${label}` : `${label} ${arrow}`),
    );
    return anchor;
  }
}

const here = currentPage();
const sidebar = document.querySelector("[data-docs='sidebar']");
const toc = document.querySelector("[data-docs='toc']");
const pager = document.querySelector("[data-docs='pager']");
const article = document.querySelector("[data-docs='article']");

if (sidebar) renderSidebar(sidebar, here);
if (toc && article) renderToc(toc, article);
if (pager) renderPrevNext(pager, here);
