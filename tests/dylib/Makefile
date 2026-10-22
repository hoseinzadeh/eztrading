


.PHONY: all clean

all:
	clang -dynamiclib plugin.cpp --std=c++11 -o libplugin.dylib -lstdc++
	c++ --std=c++11 -o cli cli.cpp -g3 -O0

clean:
	rm -f libplugin.dylib cli

distclean: clean