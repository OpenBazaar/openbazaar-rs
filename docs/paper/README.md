# OpenBazaar protocol paper

`main.tex` is the source, `sections/` holds one file per section, `figures/` holds the TikZ figures, and `openbazaar.pdf` is the built paper.

The paper describes the network as a design. How much of it the reference implementation covers, and the escrow review tracking, are in [`docs/STATUS.md`](../STATUS.md); keep commit hashes, PR numbers, and status out of the paper.

## Build

```sh
cd docs/paper
tectonic main.tex
cp main.pdf openbazaar.pdf
```

`latexmk -pdf main.tex` also works with a TeX Live install that has `natbib`, `booktabs`, `tabularx`, `longtable`, `tikz`, and `hyperref`.

## Public copy

The paper is published at https://openbazaar.github.io/openbazaar-rs/paper/ (PDF: https://openbazaar.github.io/openbazaar-rs/paper/openbazaar.pdf) from the `gh-pages` branch. To republish after committing a new `openbazaar.pdf`, run `docs/paper/publish-pages.sh` on macOS. It rebuilds the page, abstract, and preview image and pushes to `gh-pages`; the page links to the source at the current commit. The URLs stay the same.

## Submission (not yet submitted)

arXiv:

1. Run `latexmk -pdf main.tex` once with pdfLaTeX to produce `main.bbl`, since arXiv may not run BibTeX.
2. Make a tarball of `main.tex`, `main.bbl`, `sections/`, and `figures/`. Leave out `references.bib`, the PDF, and build files.
3. Submit under primary category `cs.CR`, optionally cross-listed to `cs.NI`. A first-time submitter to `cs.CR` may need an endorsement.
4. Choose a license (CC BY 4.0 suggested), paste the abstract as plain text, and check the generated PDF before announcing.

IACR Cryptology ePrint Archive:

1. Upload `openbazaar.pdf`.
2. Category: Cryptographic protocols. Keywords: Bitcoin, escrow, adaptor signatures, MuSig2, verifiable encryption, HPKE, marketplaces.
3. Enter the abstract and the author, and confirm the paper is not under submission elsewhere with a conflicting policy.
