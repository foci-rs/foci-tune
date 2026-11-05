.PHONY: installer FORCE
installer: build/foci-tune-installer.sh build/foci-tune-installer.sh.sha256

build/foci-tune-installer.sh: installer/kpi.sh installer/installer.sh FORCE
	mkdir -p build
	{ \
	  cat installer/kpi.sh; \
	  echo; \
	  cat installer/installer.sh; \
	  echo; \
	  echo 'foci_main "$$@"'; \
	} > build/foci-tune-installer.sh
	chmod +x build/foci-tune-installer.sh

build/foci-tune-installer.sh.sha256: build/foci-tune-installer.sh
	cd build && shasum -a 256 foci-tune-installer.sh > foci-tune-installer.sh.sha256

FORCE:
