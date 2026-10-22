#pragma once

#include <string>
#include <cassert>

using namespace std;
namespace json {
    inline bool is_whitespace(char c) {
        return c == ' ' || c == '\n' || c == '\t';
    }

    inline void skip_whitespace(char *&s) {
        assert(s);
        while (is_whitespace(*s)) s++;
    }

    inline bool next_char(char *&s, char ch) {
        skip_whitespace(s);
        if (*s == ch) {
            s++;
            return true;
        }
        return false;
    }

    inline bool next_curly_open(char *&s) {
        return next_char(s, '{');
    }

    inline bool next_curly_close(char *&s) {
        return next_char(s, '}');
    }

    inline bool next_bracket_open(char *&s) {
        return next_char(s, '[');
    }

    inline bool next_bracket_close(char *&s) {
        return next_char(s, ']');
    }

    inline bool next_colon(char *&s) {
        return next_char(s, ':');
    }

    inline bool next_comma(char *&s) {
        return next_char(s, ',');
    }
    
    extern bool next_string(char *&s, string &out);
    extern bool next_int(char *&s, string &out);
}
