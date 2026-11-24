.PHONY: core clean package

core:
	cd core && cargo swift package --name BarBudsCore --platforms macos
	rm -rf BarBudsCore
	mv core/BarBudsCore ./BarBudsCore

clean:
	cd core && cargo clean
	rm -rf BarBudsCore build dist

# Installable DMG in dist/. CONFIG=Release (default) or Debug.
package:
	CONFIG=$(or $(CONFIG),Release) scripts/package.sh
