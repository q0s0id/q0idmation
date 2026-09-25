"""Build the bundled offline guide with Python's standard library.

Supports the headings, fenced code, tables, lists and inline markup used by
this reference. Run with --check to detect stale generated documentation.
"""
from pathlib import Path
import argparse
import html
import re

ROOT = Path(__file__).resolve().parent
PAGES = ["README.md", "syntax.md", "builtins.md", "hosts.md", "events.md",
         "q0shell.md", "runtime.md", "examples.md"]


def slug(text):
    return re.sub(r"[^\w-]+", "-", text.lower()).strip("-")


def page_id(path):
    return "overview" if path == "README.md" else slug(Path(path).stem)


def link_target(target):
    path, _, anchor = target.partition("#")
    if path in PAGES:
        return "#" + page_id(path) + ("-" + anchor if anchor else "")
    if path.startswith("examples/") and path.endswith(".q0lang"):
        return "#example-" + slug(Path(path).stem)
    if path == "guide.html":
        return "#overview"
    return target


def inline(text):
    # Escape all prose before inserting our own markup. Code spans are opaque.
    tokens = re.split(r"(`[^`]+`|\[[^\]]+\]\([^\s)]+\)|\*\*[^*]+\*\*)", text)
    result = []
    for token in tokens:
        if token.startswith("`") and token.endswith("`"):
            result.append("<code>" + html.escape(token[1:-1]) + "</code>")
        elif token.startswith("**") and token.endswith("**"):
            result.append("<strong>" + html.escape(token[2:-2]) + "</strong>")
        elif re.fullmatch(r"\[[^\]]+\]\([^\s)]+\)", token):
            label, target = re.fullmatch(r"\[([^\]]+)\]\(([^\s)]+)\)", token).groups()
            target = link_target(target)
            if re.match(r"(?:javascript|data|vbscript):", target, re.I):
                result.append(html.escape(label))
            else:
                result.append(f'<a href="{html.escape(target, quote=True)}">{html.escape(label)}</a>')
        else:
            result.append(html.escape(token))
    return "".join(result)


def table_cells(line):
    cells, cell, code = [], [], False
    for char in line.strip().strip("|"):
        if char == "`":
            code = not code
        if char == "|" and not code:
            cells.append("".join(cell).strip())
            cell = []
        else:
            cell.append(char)
    cells.append("".join(cell).strip())
    return cells


def code_block(source, language="q0lang"):
    return ('<div class="code"><div class="codebar"><span>' + html.escape(language)
            + '</span><button type="button" class="copy">копировать</button></div><pre><code>'
            + html.escape(source) + '</code></pre></div>')


def render(source, section):
    lines, out, index = source.splitlines(), [], 0
    while index < len(lines):
        line = lines[index]
        if not line.strip():
            index += 1
            continue
        if line.startswith("```"):
            language, block = line[3:].strip(), []
            index += 1
            while index < len(lines) and not lines[index].startswith("```"):
                block.append(lines[index])
                index += 1
            out.append(code_block("\n".join(block), language))
        elif re.match(r"^#{1,6} ", line):
            level, title = line.split(" ", 1)
            tag = min(len(level) + 1, 6)
            out.append(f'<h{tag} id="{section}-{slug(title)}">{inline(title)}</h{tag}>')
        elif line.startswith("|"):
            rows = []
            while index < len(lines) and lines[index].startswith("|"):
                cells = table_cells(lines[index])
                if not all(re.fullmatch(r":?-+:?", cell.replace(" ", "")) for cell in cells):
                    tag = "th" if not rows else "td"
                    rows.append("<tr>" + "".join(f"<{tag}>{inline(cell)}</{tag}>" for cell in cells) + "</tr>")
                index += 1
            out.append('<div class="table"><table>' + "".join(rows) + "</table></div>")
            continue
        elif re.match(r"^(?:- |\d+\. )", line):
            ordered = bool(re.match(r"^\d+\. ", line))
            tag, items = ("ol" if ordered else "ul"), []
            while index < len(lines) and re.match(r"^(?:- |\d+\. )", lines[index]):
                item = re.sub(r"^(?:- |\d+\. )", "", lines[index])
                index += 1
                while index < len(lines) and lines[index].startswith("  "):
                    item += " " + lines[index].strip()
                    index += 1
                items.append("<li>" + inline(item) + "</li>")
            out.append(f"<{tag}>" + "".join(items) + f"</{tag}>")
            continue
        else:
            paragraph = [line]
            index += 1
            while index < len(lines) and lines[index].strip() and not re.match(r"^(?:#|```|\||- |\d+\. )", lines[index]):
                paragraph.append(lines[index])
                index += 1
            out.append("<p>" + inline(" ".join(paragraph)) + "</p>")
            continue
        index += 1
    return "\n".join(out)


STYLE = """
:root{color-scheme:dark;--bg:#101318;--panel:#181d25;--ink:#e3e7ed;--muted:#aab4c3;--accent:#c4dfa5;--line:#303a48}
*{box-sizing:border-box}html{scroll-behavior:smooth;scroll-padding-top:24px}body{margin:0;background:var(--bg);color:var(--ink);font:16px/1.7 'Segoe UI',sans-serif}
aside{position:fixed;inset:0 auto 0 0;width:280px;padding:32px 24px;overflow:auto;background:var(--panel);border-right:1px solid var(--line)}
.brand{font-size:28px;font-weight:650;letter-spacing:-1px;margin-bottom:0}.subtitle{color:var(--muted);font-size:13px;margin-bottom:24px}nav a{display:block;padding:7px 0;color:var(--muted);text-decoration:none}nav a:hover{color:var(--accent)}
input{width:100%;margin:0 0 18px;padding:11px 12px;border:1px solid var(--line);border-radius:8px;background:var(--bg);color:var(--ink);font:inherit}input:focus{outline:2px solid var(--accent)}
main{max-width:1300px;margin-left:280px;padding:42px 64px 90px}header{padding:0 0 24px;border-bottom:1px solid var(--line)}h1{font-size:44px;line-height:1.15;letter-spacing:-1.5px;margin:12px 0}h2{font-size:30px;line-height:1.3;margin:0 0 24px}h3{font-size:22px;margin:38px 0 12px}h4{font-size:18px}
article{padding:44px 0;border-bottom:1px solid var(--line)}a{color:var(--accent);text-underline-offset:3px}p{max-width:85ch}li{padding:3px 0}code{font:14px/1.7 Consolas,'Courier New',monospace;background:var(--panel);border-radius:4px;padding:2px 5px;overflow-wrap:anywhere}
.code{margin:22px 0;background:var(--panel);border:1px solid var(--line);border-radius:10px;overflow:hidden}.codebar{display:flex;justify-content:space-between;align-items:center;padding:8px 14px;border-bottom:1px solid var(--line);font-size:12px;color:var(--muted)}button{background:transparent;color:var(--accent);border:1px solid var(--line);padding:4px 10px;border-radius:5px;cursor:pointer}button:focus-visible,a:focus-visible{outline:2px solid var(--accent)}pre{padding:18px;margin:0;overflow:auto}pre code{padding:0;white-space:pre;overflow-wrap:normal}
.table{overflow:auto;margin:20px 0}table{width:100%;border-collapse:collapse;font-size:14px}td,th{text-align:left;vertical-align:top;padding:11px 14px;border:1px solid var(--line)}th{background:var(--panel);font-weight:600}.muted{color:var(--muted)}#empty{display:none;padding:30px 0}
@media(max-width:850px){aside{position:static;width:auto;border-right:0;border-bottom:1px solid var(--line)}nav{display:flex;gap:10px 22px;flex-wrap:wrap}main{margin:0;padding:28px 22px}h1{font-size:34px}}
@media print{aside,.codebar button,header,#empty{display:none}main{margin:0;max-width:none;padding:0}body{background:white;color:black}a{color:inherit}article{break-before:page}.code,code,th{background:#f5f5f5}pre{white-space:pre-wrap}}
"""
SCRIPT = """
const input=document.querySelector('#search');
input.addEventListener('input',()=>{const query=input.value.toLocaleLowerCase().trim();let shown=0;document.querySelectorAll('article').forEach(article=>{article.hidden=query&&!article.textContent.toLocaleLowerCase().includes(query);if(!article.hidden)shown++});document.querySelector('#empty').style.display=shown?'none':'block'});
document.querySelectorAll('a[href^="#"]').forEach(link=>link.addEventListener('click',()=>{input.value='';input.dispatchEvent(new Event('input'))}));
document.querySelectorAll('.copy').forEach(button=>button.addEventListener('click',async()=>{const code=button.closest('.code').querySelector('pre code');try{await navigator.clipboard.writeText(code.textContent);button.textContent='скопировано';setTimeout(()=>button.textContent='копировать',1500)}catch{const range=document.createRange();range.selectNodeContents(code);const selection=getSelection();selection.removeAllRanges();selection.addRange(range);button.textContent='ctrl+c: копировать выделенное'}}));
"""


def build():
    nav, articles = [], []
    for name in PAGES:
        source = (ROOT / name).read_text(encoding="utf-8")
        section = page_id(name)
        title = source.splitlines()[0].lstrip("# ").replace("q0lang: ", "")
        nav.append(f'<a href="#{section}">{html.escape(title)}</a>')
        articles.append(f'<article id="{section}">{render(source, section)}</article>')
    for example in sorted((ROOT / "examples").glob("*.q0lang")):
        articles.append(f'<article id="example-{slug(example.stem)}"><h2>{html.escape(example.name)}</h2>'
                        + code_block(example.read_text(encoding="utf-8")) + "</article>")
    return ('<!doctype html><html lang="ru"><head><meta charset="utf-8">'
            '<meta name="viewport" content="width=device-width,initial-scale=1">'
            '<title>q0lang · справочник</title><style>' + STYLE + '</style></head><body>'
            '<aside><div class="brand">q0lang</div><div class="subtitle">язык для q0idmation</div>'
            '<label for="search" class="muted">поиск по разделам</label><input id="search" type="search" placeholder="например, range или q0rig">'
            '<nav aria-label="разделы">' + "".join(nav) + '</nav></aside><main>'
            '<header><div class="muted">справочник языка</div><h1>от первого скрипта<br>до живой сцены.</h1>'
            '<p class="muted">синтаксис, библиотеки, среда исполнения и готовые примеры. весь справочник доступен без интернета.</p></header>'
            '<p id="empty" role="status">совпадений нет. попробуй другое слово.</p>'
            + "".join(articles) + '</main><script>' + SCRIPT + '</script></body></html>\n')


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    target, content = ROOT / "guide.html", build()
    if args.check:
        if not target.exists() or target.read_text(encoding="utf-8") != content:
            raise SystemExit("guide.html is stale; run python docs/q0lang/build_guide.py")
        print("offline guide is current")
    else:
        target.write_text(content, encoding="utf-8", newline="\n")
        print(target)
