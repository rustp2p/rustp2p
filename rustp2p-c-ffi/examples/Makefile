CC=gcc
CFLAGS=-Wall -Wextra -I..
LDFLAGS=-L../../target/release -lrustp2p_c_ffi -lpthread -ldl -lm
LDPATH=LD_LIBRARY_PATH=../../target/release

.PHONY: all clean test

all: simple_example test_ffi

simple_example: simple_example.c ../rustp2p.h
	$(CC) $(CFLAGS) simple_example.c -o simple_example $(LDFLAGS)

test_ffi: test_ffi.c ../rustp2p.h
	$(CC) $(CFLAGS) test_ffi.c -o test_ffi $(LDFLAGS)

test: test_ffi
	$(LDPATH) ./test_ffi

clean:
	rm -f simple_example test_ffi
