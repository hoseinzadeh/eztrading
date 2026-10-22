#include <json.h>

bool json::next_string(char *&s, string &out) {
    skip_whitespace(s);
    if (*s == '"') {
        s++;
        out = "";
        bool escape=false;
        for(;*s;s++) {
            if (*s == '\\') escape = true;
            else if (escape) escape = false;
            else if (*s == '"') break;
            else out += *s;
        }
        s++;
        return true;
    }
    return false;
}

bool json::next_int(char *&s, string &out) {
    skip_whitespace(s);
    if (*s >= '0' && *s <= '9') {
        out = "";
        for(;*s;s++) {
            if (is_whitespace(*s) || *s == ',' || *s == '}') break;
            else out += *s;
        }
        return true;
    }
    return false;
}