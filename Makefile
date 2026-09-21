# maskrun — single-file tool, so these are conveniences rather than a build.
PREFIX ?= $(HOME)/.local
BINDIR ?= $(PREFIX)/bin
PYTHON ?= python3

.PHONY: help test install uninstall guard lint

help:
	@echo "make test       run the test suite"
	@echo "make install    copy bin/maskrun to $(BINDIR)"
	@echo "make uninstall  remove it again"
	@echo "make guard      register the agent guard hook"
	@echo "make lint       byte-compile and shellcheck"

test:
	$(PYTHON) test/test_maskrun.py

install:
	@mkdir -p $(BINDIR)
	install -m 755 bin/maskrun $(BINDIR)/maskrun
	@echo "installed -> $(BINDIR)/maskrun"
	@command -v maskrun >/dev/null 2>&1 || \
		echo "note: $(BINDIR) is not on your PATH"

uninstall:
	rm -f $(BINDIR)/maskrun
	@echo "removed $(BINDIR)/maskrun"

guard:
	$(PYTHON) bin/maskrun install-guard

lint:
	$(PYTHON) -m py_compile bin/maskrun test/test_maskrun.py
	@command -v shellcheck >/dev/null 2>&1 && shellcheck --shell=sh install.sh || \
		echo "shellcheck not installed, skipped"
