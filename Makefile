
CC=c++
CFLAGS=-std=c++20 -g3 -O0 -pthread
CARGO=cargo
CARGOFLAGS=run --quiet --bin=compile --package=compile
CLANG=clang
LIBS=
HEADERDIR=package/inc
RUSTFLAGS=-Awarnings
SMLLIBS=package/libs
EZTFILE=tests/example.sm
TEST=tests/test.cpp
SCHEDULER=package/common/scheduler.cpp
CLANGFLAGS=-dynamiclib -std=c++20 -lstdc++
UNAME_S := $(shell uname -s)
EZTFILES=$(EZTFILE) $(shell find $(SMLLIBS) -name '*.sm')
BUILDDIR=.
SOURCEDIR=package/common
SOURCES := $(shell find $(SOURCEDIR) -name '*.cpp')
COMMON_OBJECTS := $(addprefix $(BUILDDIR)/,$(SOURCES:%.cpp=%.o))

SOURCES += $(EZTFILES:%.sm=%.cpp)
LIBRARY=$(EZTFILE:%.sm=%.dylib)
SCH_LIB=$(SCHEDULER:%.cpp=%.dylib)

ifeq ($(UNAME_S),Linux)
	CFLAGS+=-fPIC
	CLANGFLAGS=-shared -ldl -fPIC -std=c++20 -lstdc++ -pthread
	LIBS=-ldl
	LIBRARY=$(EZTFILE:%.sm=%.so)
	SCH_LIB=$(SCHEDULER:%.cpp=%.so)
endif

# Get list of object files, with paths
OBJECTS := $(addprefix $(BUILDDIR)/,$(SOURCES:%.cpp=%.o))

.PHONY: all test scheduler cargo clean

all: $(LIBRARY) test

$(LIBRARY): cargo $(OBJECTS)
	$(CLANG) $(CLANGFLAGS) $(OBJECTS) -o $(LIBRARY) $(LIBS)

$(BUILDDIR)/%.o: %.cpp
	$(CC) $(CFLAGS) -I$(HEADERDIR) -I. -c $< -o $@ $(LIBS)

cargo: $(EZTFILES)
	RUSTFLAGS=-Awarnings $(CARGO) $(CARGOFLAGS) -- -I $(SMLLIBS) $<
	$(CC) $(CFLAGS) -I$(HEADERDIR) -I. -c $(<:%.sm=%.cpp) -o $(<:%.sm=%.o)

scheduler: $(COMMON_OBJECTS)
	$(CC) $(CFLAGS) -DSCHEDULER_INTERFACE -I$(HEADERDIR) -I. -c $(SCHEDULER) -o $(SCHEDULER:%.cpp=%.o) $(LIBS)
	$(CLANG) $(CLANGFLAGS) $(COMMON_OBJECTS) -o $(SCH_LIB) $(LIBS)

%.sm:
	RUSTFLAGS=-Awarnings $(CARGO) $(CARGOFLAGS) -- -I $(SMLLIBS) $<

test: $(TEST)
	$(CC) $(CFLAGS) -I$(HEADERDIR) -I. $< $(COMMON_OBJECTS) -o $(<:%.cpp=%) $(LIBS)

clean:
	rm -f $(LIBRARY) $(OBJECTS) $(EZTFILES:%.sm=%.cpp) $(EZTFILES:%.sm=%.h) $(EZTFILES:%.sm=%.smo) $(TEST:%.cpp=%) $(SCH_LIB)

distclean: clean
