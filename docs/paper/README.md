# OpenBazaar protocol paper

`main.tex` is the source, `sections/` holds one file per section, `figures/` holds the TikZ figures, and `openbazaar.pdf` is the built paper.

The commits the paper describes are set in `sections/review-status.tex` (`\paycommit`, `\reviewcommit`), along with the status of each escrow review finding. Update them there when PR #2 or PR #3 moves, and rerun the tests before changing a status.

## Build

```sh
cd docs/paper
tectonic main.tex
cp main.pdf openbazaar.pdf
```

`latexmk -pdf main.tex` also works with a TeX Live install that has `natbib`, `booktabs`, `tabularx`, `longtable`, `tikz`, and `hyperref`.

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
