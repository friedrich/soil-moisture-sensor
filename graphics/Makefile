graphics = sensor-circuit.png

graphics: $(graphics)

%.png: %.pdf
	convert -density 600 $< $@

%.pdf: %.tex
	latexmk -norc -pdf $<

.PHONY: clean
clean:
	rm -rf *.ps *.dvi *.out *.aux *.synctex.gz *.fls *.log *.fdb_latexmk *.bbl *.toc *.blg auto/ *.png *.pdf
