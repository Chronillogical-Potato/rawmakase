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
	cargo clippy --locked --all-targets --no-default-features -- -D warnings
	cargo test --locked
	cargo fmt --manifest-path tools/rawmakase-ctl/Cargo.toml --check
	cargo clippy --manifest-path tools/rawmakase-ctl/Cargo.toml --all-targets --locked -- -D warnings
	cargo test --manifest-path tools/rawmakase-ctl/Cargo.toml --locked

# Install does not rebuild, so `make && sudo make install PREFIX=/usr` never compiles as root.
install:
	@test -x $(BIN) || { echo "Run 'make' first to build $(BIN)"; exit 1; }
	install -Dm755 $(BIN) $(DESTDIR)$(PREFIX)/bin/rawmakase
	install -Dm644 LICENSE $(DESTDIR)$(PREFIX)/share/licenses/$(PKGNAME)/LICENSE
	install -Dm644 licenses/Adobe-DNG-SDK.txt $(DESTDIR)$(PREFIX)/share/licenses/$(PKGNAME)/Adobe-DNG-SDK.txt
	install -Dm644 licenses/Hack-MIT-BitstreamVera.txt $(DESTDIR)$(PREFIX)/share/licenses/$(PKGNAME)/Hack-MIT-BitstreamVera.txt
	install -Dm644 licenses/Inter-OFL.txt $(DESTDIR)$(PREFIX)/share/licenses/$(PKGNAME)/Inter-OFL.txt
	install -Dm644 licenses/Lucide-ISC.txt $(DESTDIR)$(PREFIX)/share/licenses/$(PKGNAME)/Lucide-ISC.txt
	install -Dm644 licenses/NotoEmoji-OFL.txt $(DESTDIR)$(PREFIX)/share/licenses/$(PKGNAME)/NotoEmoji-OFL.txt
	install -Dm644 licenses/Ubuntu-UFL.txt $(DESTDIR)$(PREFIX)/share/licenses/$(PKGNAME)/Ubuntu-UFL.txt
	install -Dm644 licenses/emoji-icon-font-MIT.txt $(DESTDIR)$(PREFIX)/share/licenses/$(PKGNAME)/emoji-icon-font-MIT.txt
ifneq ($(shell uname -s),Darwin)
	install -Dm644 packaging/applications/rawmakase.desktop $(DESTDIR)$(PREFIX)/share/applications/rawmakase.desktop
	install -Dm644 packaging/icons/rawmakase.svg $(DESTDIR)$(PREFIX)/share/icons/hicolor/scalable/apps/rawmakase.svg
endif

uninstall:
	rm -f $(DESTDIR)$(PREFIX)/bin/rawmakase
	rm -f $(DESTDIR)$(PREFIX)/share/applications/rawmakase.desktop
	rm -f $(DESTDIR)$(PREFIX)/share/icons/hicolor/scalable/apps/rawmakase.svg
	rm -rf $(DESTDIR)$(PREFIX)/share/licenses/$(PKGNAME)
