#include <contents.h>
#include <json.h>

using namespace __contents;

any __contents::any::from_json(char *&jstr) {
    any res;
    if (json::next_curly_open(jstr)) {
        std::string buf;
        while (json::next_string(jstr, buf)) {
            if (buf == "size") {
                if (json::next_colon(jstr)) {
                    if (json::next_int(jstr, buf)) {
                        res.size = stoi(buf.data());
                    } else {
                        assert(false && "expected an integer value");
                    }
                }
            } else if (buf == "data") {
                if (json::next_colon(jstr)) {
                    if (json::next_string(jstr, buf)) {
                        char *hex = buf.data();
                        char e;
                        for (int i=0; i<buf.size(); i+=2) {
                            e = hex[i+2];
                            hex[i+2] = '\0';
                            res.data[i >> 1] = (uint8_t)strtol(hex+i, nullptr, 16);
                            hex[i+2] = e;
                        }
                    } else {
                        assert(false && "expected a string value");
                    }
                }
            } else {
                assert(false && "unknown key");
            }
            if (!json::next_comma(jstr)) break;
        }
        assert(json::next_curly_close(jstr));
    }
    return res;
}

param __contents::param::from_json(char *&jstr) {
    param res("");
    if (json::next_curly_open(jstr)) {
        std::string buf;
        while (json::next_string(jstr, buf)) {
            if (buf == "id") {
                if (json::next_colon(jstr)) {
                    if (json::next_string(jstr, buf)) {
                        res.id = buf;
                    } else {
                        assert(false && "expected a string value");
                    }
                }
            } else if (buf == "name") {
                if (json::next_colon(jstr)) {
                    if (json::next_string(jstr, buf)) {
                        res.name = buf;
                    } else {
                        assert(false && "expected a string value");
                    }
                }
            } else if (buf == "type") {
                if (json::next_colon(jstr)) {
                    if (json::next_string(jstr, buf)) {
                        res.type = buf;
                    } else {
                        assert(false && "expected a string value");
                    }
                }
            } else if (buf == "value") {
                if (json::next_colon(jstr)) {
                    res.value = any::from_json(jstr);
                }
            } else if (buf == "options") {
                if (json::next_colon(jstr)) {
                    if (json::next_curly_open(jstr)) {
                        if(json::next_string(jstr, buf)) {
                            if (buf == "List") {
                                if (json::next_colon(jstr)) {
                                    if (json::next_bracket_open(jstr)) {
                                        while (true) {
                                            if (json::next_curly_open(jstr)) {
                                                while (json::next_string(jstr, buf)) {
                                                    if (buf == "name") {
                                                        if (json::next_colon(jstr)) {
                                                            if (json::next_string(jstr, buf)) {
                                                                res.option_strings.push_back(buf);
                                                            } else {
                                                                assert(false && "expected a string value");
                                                            }
                                                        }
                                                    } else if (buf == "value") {
                                                        if (json::next_colon(jstr)) {
                                                            res.option_values.push_back(any::from_json(jstr));
                                                        }
                                                    } else {
                                                        assert(false && "unknown key");
                                                    }
                                                    if (!json::next_comma(jstr)) break;
                                                }
                                                assert(json::next_curly_close(jstr));
                                            }
                                            if (!json::next_comma(jstr)) break;
                                        }
                                        assert(json::next_bracket_close(jstr));
                                    } else {
                                        assert(false && "expected an array");
                                    }
                                }
                            } else if (buf == "Range") {
                                res.options_is_range = true;
                                if (json::next_colon(jstr)) {
                                    if (json::next_curly_open(jstr)) {
                                        while (json::next_string(jstr, buf)) {
                                            if (buf == "hi") {
                                                if (json::next_colon(jstr)) {
                                                    if (json::next_int(jstr, buf)) {
                                                        res.options_high = atof(buf.c_str());
                                                    }
                                                }
                                            } else if (buf == "lo") {
                                                if (json::next_colon(jstr)) {
                                                    if (json::next_int(jstr, buf)) {
                                                        res.options_low = atof(buf.c_str());
                                                    }
                                                }
                                            } else {
                                                assert(false && "unknown key");
                                            }
                                            if (!json::next_comma(jstr)) break;
                                        }
                                        assert(json::next_curly_close(jstr));
                                    }
                                }
                            } else {
                                assert(false && "unknown key");
                            }
                        } 
                        assert(json::next_curly_close(jstr));
                    } else if (json::next_string(jstr, buf)) {
                        assert(buf == "None" && "unknown value");
                    }
                }
            } else {
                assert(false && "unknown key");
            }
            if (!json::next_comma(jstr)) break;
        }
        assert(json::next_curly_close(jstr));
    }
    return res;
}

machine __contents::machine::from_json(char *&jstr) {
    machine res("");
    if (json::next_curly_open(jstr)) {
        std::string buf;
        while (json::next_string(jstr, buf)) {
            if (buf == "name") {
                if (json::next_colon(jstr)) {
                    if (json::next_string(jstr, buf)) {
                        res.name = buf;
                    } else {
                        assert(false && "expected a string value");
                    }
                }
            } else if (buf == "params") {
                if (json::next_colon(jstr)) {
                    if (json::next_bracket_open(jstr)) {
                        while (true) {
                            auto p = param::from_json(jstr);
                            if (!p.name.empty()) res.params.push_back(p);
                            if (!json::next_comma(jstr)) break;
                        }
                        assert(json::next_bracket_close(jstr));
                    } else {
                        assert(false && "expected an array");
                    }
                }
            } else {
                assert(false && "unknown key");
            }
            if (!json::next_comma(jstr)) break;
        }
        assert(json::next_curly_close(jstr));
    }
    return res;
}

list __contents::list::from_json(char *&jstr) {
    list res;
    if (json::next_curly_open(jstr)) {
        std::string buf;
        if (json::next_string(jstr, buf)) {
            if (buf == "machines") {
                if (json::next_colon(jstr)) {
                    if (json::next_bracket_open(jstr)) {
                        while (true) {
                            auto m = machine::from_json(jstr);
                            if (!m.name.empty()) res.add(m);
                            if (!json::next_comma(jstr)) break;
                        }
                        assert(json::next_bracket_close(jstr));
                    }
                }
            } else {
                assert(false && "unknown key");
            }
        }
        assert(json::next_curly_close(jstr));
    }
    return res;
}

std::unordered_map<std::string, __contents::any> __contents::params_from_json(char *machine_json) {
    std::unordered_map<std::string, __contents::any> params;
    if (machine_json) {
        auto m = machine::from_json(machine_json);
        for (auto p: m.params) {
            params.insert_or_assign(p.id, p.value);
        }
    }
    return params;
}