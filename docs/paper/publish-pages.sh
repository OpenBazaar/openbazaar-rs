#!/usr/bin/env bash
# Publish openbazaar.pdf to the gh-pages branch, served at $PAGES_URL/paper/.
# Requires macOS (qlmanage, sips) and push access to origin.
set -euo pipefail

paper_dir=$(cd "$(dirname "$0")" && pwd)
repo_root=$(git -C "$paper_dir" rev-parse --show-toplevel)
remote_url=$(git -C "$repo_root" remote get-url origin)
repo_slug=$(printf '%s' "$remote_url" | sed -E 's#(\.git)?$##; s#^.*github\.com[:/]##')
pages_url=${PAGES_URL:-https://openbazaar.github.io/openbazaar-rs}
pdf="$paper_dir/openbazaar.pdf"

if ! git -C "$repo_root" diff --quiet HEAD -- "$paper_dir"; then
  echo "docs/paper has uncommitted changes; commit them first" >&2
  exit 1
fi
commit=$(git -C "$repo_root" rev-parse HEAD)
short=${commit:0:7}

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
site="$work/site"
if git ls-remote --exit-code --heads "$remote_url" gh-pages >/dev/null; then
  git clone -q --depth 1 --branch gh-pages --single-branch "$remote_url" "$site"
else
  git init -q "$site"
  git -C "$site" checkout -q --orphan gh-pages
  git -C "$site" remote add origin "$remote_url"
fi
mkdir -p "$site/paper"

latex_to_text() {
  sed -E 's/\\(begin|end)\{abstract\}//; s/---/—/g; s/--/–/g; s/~/ /g; s/\\([%&$#_])/\1/g' |
    sed -E 's/&/\&amp;/g; s/</\&lt;/g; s/>/\&gt;/g; s/"/\&quot;/g'
}

title=$(sed -nE 's/^\\title\{(.*)\}$/\1/p' "$paper_dir/main.tex" | sed -E 's/\\\\/ /g' | latex_to_text)
abstract=$(latex_to_text <"$paper_dir/sections/abstract.tex")
abstract_html=$(printf '%s\n' "$abstract" | awk 'NF { printf "    <p>%s</p>\n", $0 }')
description=$(printf '%s\n' "$abstract" | awk 'NF { print; exit }' |
  awk '{ if (length($0) <= 280) { print; exit } s = substr($0, 1, 280); sub(/ [^ ]*$/, "", s); print s "…" }')

cp "$pdf" "$site/paper/openbazaar.pdf"

qlmanage -t -s 2000 -o "$work" "$pdf" >/dev/null 2>&1
sips --resampleWidth 1200 "$work/openbazaar.pdf.png" --out "$site/paper/preview.png" >/dev/null
sips -c 628 1200 --cropOffset 150 0 "$site/paper/preview.png" >/dev/null

cat >"$site/paper/index.html" <<EOF
<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>$title</title>
  <meta name="description" content="$description">
  <link rel="canonical" href="$pages_url/paper/">
  <meta property="og:type" content="article">
  <meta property="og:url" content="$pages_url/paper/">
  <meta property="og:title" content="$title">
  <meta property="og:description" content="$description">
  <meta property="og:image" content="$pages_url/paper/preview.png?v=$short">
  <meta property="og:image:width" content="1200">
  <meta property="og:image:height" content="628">
  <meta name="twitter:card" content="summary_large_image">
  <meta name="twitter:title" content="$title">
  <meta name="twitter:description" content="$description">
  <meta name="twitter:image" content="$pages_url/paper/preview.png?v=$short">
  <style>
    body { max-width: 42rem; margin: 3rem auto; padding: 0 1.25rem; font: 1.05rem/1.6 Georgia, "Times New Roman", serif; color: #1a1a1a; }
    h1 { font-size: 1.6rem; line-height: 1.3; font-weight: normal; }
    h2 { font-size: 1rem; text-transform: uppercase; letter-spacing: .08em; margin-top: 2rem; }
    .author { color: #555; }
    .download { display: inline-block; margin: 1rem 0; padding: .6rem 1.1rem; background: #1a1a1a; color: #fff; text-decoration: none; border-radius: 4px; font-family: system-ui, sans-serif; }
    footer { margin-top: 3rem; color: #666; font-size: .85rem; font-family: system-ui, sans-serif; }
    a { color: inherit; }
  </style>
</head>
<body>
  <h1>$title</h1>
  <p class="author">Brian Hoffman · OpenBazaar project</p>
  <a class="download" href="openbazaar.pdf">Download PDF</a>
  <h2>Abstract</h2>
$abstract_html
  <footer><a href="https://github.com/$repo_slug/tree/$commit/docs/paper">Source</a></footer>
</body>
</html>
EOF

cat >"$site/index.html" <<EOF
<!doctype html>
<meta charset="utf-8">
<title>OpenBazaar</title>
<meta http-equiv="refresh" content="0; url=paper/">
<link rel="canonical" href="$pages_url/paper/">
<a href="paper/">OpenBazaar protocol paper</a>
EOF
touch "$site/.nojekyll"

git -C "$site" add -A
if git -C "$site" diff --cached --quiet; then
  echo "Nothing changed; $pages_url/paper/ is already up to date."
  exit 0
fi
git -C "$site" commit -q -m "Publish paper from $short"
git -C "$site" push -q origin gh-pages
echo "Pushed. Live in a minute or two at $pages_url/paper/"
