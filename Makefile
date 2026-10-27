SHELL := /bin/bash
BIN   := tickerstrip
PREFIX?= $(HOME)/.local
PLIST := $(HOME)/Library/LaunchAgents/com.tickerstrip.plist

.PHONY: build release test run install uninstall autostart unautostart snapshot clean fmt

build:
	cargo build

release:
	cargo build --release

test:
	cargo test

run: build
	./target/debug/$(BIN)

# 前台跑 + 每秒打印几何信息
debug: build
	MBQUOTE_DEBUG=1 ./target/debug/$(BIN)

# 把面板渲染成 PNG，用于核对排版（不依赖录屏权限）
snapshot: build
	rm -f /tmp/tickerstrip-panel.png
	MBQUOTE_DEBUG_SNAPSHOT=/tmp/tickerstrip-panel.png ./target/debug/$(BIN) &
	sleep 4; pkill -f 'target/debug/$(BIN)' || true
	@echo "面板快照：/tmp/tickerstrip-panel.png"
	@open /tmp/tickerstrip-panel.png

# README 示意图：用一份 demo 自选渲染，不动你自己的配置（也不泄露你的持仓）
demo-config:
	python3 tools/make_demo_config.py /tmp/tickerstrip-shot/config.json

shot: build demo-config
	rm -f /tmp/tickerstrip-shot/raw-*.png*
	@TICKERSTRIP_CONFIG=/tmp/tickerstrip-shot/config.json \
	  MBQUOTE_DEBUG_SHOT=/tmp/tickerstrip-shot ./target/debug/$(BIN) & echo $$! > /tmp/tickerstrip-shot.pid
	@sleep 7
	@kill $$(cat /tmp/tickerstrip-shot.pid) 2>/dev/null || true
	@rm -f /tmp/tickerstrip-shot.pid
	python3 tools/make_shot.py /tmp/tickerstrip-shot docs/screenshot.png
	@open docs/screenshot.png

# 图标：assets/icon-src.png 是源图（256x256 也行），放大到 1024 并出 .icns
icon:
	python3 tools/make_icon.py

# 打两个架构的发行包：tar.gz（命令行用户）+ dmg（拖进 Applications）
dist: icon
	rustup target add aarch64-apple-darwin >/dev/null 2>&1 || true
	@VERSION=$$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1); \
	echo "版本 $$VERSION"; \
	for spec in "x86_64-apple-darwin x86_64" "aarch64-apple-darwin arm64"; do \
	  set -- $$spec; target=$$1; arch=$$2; \
	  echo "== $$target"; \
	  cargo build --release --target $$target || exit 1; \
	  mkdir -p dist; \
	  d=dist/tickerstrip-$$target; rm -rf $$d; mkdir -p $$d; \
	  cp target/$$target/release/$(BIN) $$d/; cp README.md LICENSE $$d/; \
	  tar -C dist -czf dist/tickerstrip-$$target.tar.gz tickerstrip-$$target; \
	  rm -rf $$d; \
	  python3 tools/make_dmg.py \
	    --binary target/$$target/release/$(BIN) \
	    --arch $$arch --version $$VERSION \
	    --out dist/tickerstrip-$$VERSION-$$arch.dmg; \
	done; \
	cd dist && shasum -a 256 *.tar.gz *.dmg > SHA256SUMS && cat SHA256SUMS

install: release
	mkdir -p $(PREFIX)/bin
	install -m 755 target/release/$(BIN) $(PREFIX)/bin/$(BIN)
	@echo "已安装到 $(PREFIX)/bin/$(BIN)"
	@case ":$$PATH:" in *":$(PREFIX)/bin:"*) ;; *) echo "提示：把 $(PREFIX)/bin 加进 PATH";; esac

uninstall:
	-$(MAKE) unautostart
	rm -f $(PREFIX)/bin/$(BIN)

autostart: install
	@mkdir -p $(dir $(PLIST))
	@printf '%s\n' \
	  '<?xml version="1.0" encoding="UTF-8"?>' \
	  '<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">' \
	  '<plist version="1.0"><dict>' \
	  '  <key>Label</key><string>com.tickerstrip</string>' \
	  '  <key>ProgramArguments</key>' \
	  '  <array><string>$(PREFIX)/bin/$(BIN)</string></array>' \
	  '  <key>RunAtLoad</key><true/>' \
	  '  <key>KeepAlive</key><true/>' \
	  '  <key>StandardErrorPath</key><string>/tmp/tickerstrip.err.log</string>' \
	  '</dict></plist>' > $(PLIST)
	-launchctl unload $(PLIST) 2>/dev/null
	launchctl load $(PLIST)
	@echo "已设为登录自启（$(PLIST)）"

unautostart:
	-launchctl unload $(PLIST) 2>/dev/null
	rm -f $(PLIST)
	@echo "已取消登录自启"

fmt:
	cargo fmt

clean:
	cargo clean
