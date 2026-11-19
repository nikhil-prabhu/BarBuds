.PHONY: core clean

core:
	cd core && cargo swift package --name BarBudsCore --platforms macos
	rm -rf BarBudsCore
	mv core/BarBudsCore ./BarBudsCore

clean:
	cd core && cargo clean
	rm -rf BarBudsCore