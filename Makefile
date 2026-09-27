PREFIX ?= $(HOME)/.local
# Package name for the license directory (Arch packages pass their pkgname).
PKGNAME ?= rawmakase
BIN := target/release/rawmakase

.PHONY: all build check install uninstall
all: build

build:
	cargo build --release --locked

check:
	cargo fmt --check
	cargo clippy --locked --all-targets -- -D warnings
	cargo test --locked

# Install does not rebuild, so `make && sudo make install PREFIX=/usr` never compiles as root.
install:
	@test -x $(BIN) || { echo "Run 'make' first to build $(BIN)"; exit 1; }
	install -Dm755 $(BIN) $(DESTDIR)$(PREFIX)/bin/rawmakase
	install -Dm644 LICENSE $(DESTDIR)$(PREFIX)/share/licenses/$(PKGNAME)/LICENSE
	install -Dm644 licenses/Adobe-DNG-SDK.txt $(DESTDIR)$(PREFIX)/share/licenses/$(PKGNAME)/Adobe-DNG-SDK.txt
	install -Dm644 licenses/Inter-OFL.txt $(DESTDIR)$(PREFIX)/share/licenses/$(PKGNAME)/Inter-OFL.txt
ifneq ($(shell uname -s),Darwin)
	install -Dm644 packaging/rawmakase.desktop $(DESTDIR)$(PREFIX)/share/applications/rawmakase.desktop
	install -Dm644 packaging/rawmakase.svg $(DESTDIR)$(PREFIX)/share/icons/hicolor/scalable/apps/rawmakase.svg
endif

uninstall:
	rm -f $(DESTDIR)$(PREFIX)/bin/rawmakase
	rm -f $(DESTDIR)$(PREFIX)/share/applications/rawmakase.desktop
	rm -f $(DESTDIR)$(PREFIX)/share/icons/hicolor/scalable/apps/rawmakase.svg
	rm -rf $(DESTDIR)$(PREFIX)/share/licenses/$(PKGNAME)
