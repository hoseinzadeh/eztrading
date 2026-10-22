#include <cstdint>
#include <vector>
#include <string>
#include <unordered_map>
#include <json.h>
#include <cassert>
#include <cstring>

namespace __contents {
    struct any {
        uint8_t data[128];
        int size;
        std::string to_json() const {
            std::string jstr = "{\"size\": ";
            char buf[16];
            snprintf(buf, 16, "%d", size);
            jstr += buf;
            jstr += ", \"data\": \"";
            for (int i=0; i<size; i++) {
                snprintf(buf, 16, "%02x", data[i]);
                jstr += buf;
            }
            jstr += "\"}";
            return jstr;
        }
        static any from_json(char *&jstr);
    };

    template <typename T>
    static any mk_any(T obj) {
        static_assert(sizeof(T) <= 128);
        any a;
        std::memcpy((void*)a.data, (void*)(&obj), sizeof(T));
        a.size = sizeof(T);
        return a;
    }

    template <typename T>
    static T from_any(any a) {
        T r;
        std::memcpy((void*)(&r), (void*)(a.data), a.size);
        return r;
    }

    /// Symbol lists cannot cross the process boundary as raw memory, so they travel as
    /// comma-separated text ("AAPL,MSFT") inside the 128 bytes of an `any`.
    template <>
    inline any mk_any<std::vector<std::string>>(std::vector<std::string> list) {
        any a;
        std::string text;
        for (auto &s: list) { if (!text.empty()) text += ','; text += s; }
        if (text.size() > sizeof(a.data)) text.resize(sizeof(a.data));
        std::memset(a.data, 0, sizeof(a.data));
        std::memcpy(a.data, text.data(), text.size());
        a.size = (int)text.size();
        return a;
    }

    template <>
    inline std::vector<std::string> from_any<std::vector<std::string>>(any a) {
        std::vector<std::string> list;
        std::string cur;
        for (int i = 0; i < a.size && i < (int)sizeof(a.data); i++) {
            char c = (char)a.data[i];
            if (c == ',') { if (!cur.empty()) list.push_back(cur); cur.clear(); }
            else cur += c;
        }
        if (!cur.empty()) list.push_back(cur);
        return list;
    }

    struct param {
        std::string id;
        std::string name;
        std::string type;
        any value;
        std::vector<std::string> option_strings;
        std::vector<any> option_values;
        float options_low;
        float options_high;
        bool options_is_range;
        param(const char *id="", const char *name=""): id(id), name(name), options_is_range(false) {}
        std::string to_json() const {
            std::string jstr = "{\"id\": \"";
            jstr += id;
            jstr += "\", \"name\": \"";
            jstr += name;
            jstr += "\", \"type\": \"";
            jstr += type;
            jstr += "\", \"value\": ";
            jstr += value.to_json();
            jstr += ", \"options\": ";
            if (options_is_range) {
                char buf[64];
                snprintf(buf, 64, "{\"lo\": %f, \"hi\": %f}", options_low, options_high);
                jstr += "{\"Range\": ";
                jstr += buf;
                jstr += "}";
            } else if (!option_strings.empty()) {
                jstr += "{\"List\": [";
                bool first = true;
                for (int i=0; i<option_strings.size(); i++) {
                    if (!first) jstr += ",";
                    jstr += "{\"name\": \"";
                    jstr += option_strings[i];
                    jstr += "\", \"value\": ";
                    jstr += option_values[i].to_json();
                    jstr += "}";
                    first = false;
                }
                jstr += "]}";
            } else {
                jstr += "\"None\"";
            }
            jstr += "}";
            return jstr;
        }
        static param from_json(char *&jstr);
    };

    struct machine {
        std::string name;
        std::vector<param> params;
        machine(const char *name) : name(name) {}
        std::string to_json() const {
            std::string jstr = "{\"name\": \"";
            jstr += name;
            jstr += "\"";
            jstr += ", \"params\": [";
            bool first = true;
            for (auto p: params) {
                if (!first) jstr += ", ";
                jstr += p.to_json();
                first = false;
            }
            jstr += "]";
            jstr += "}";
            return jstr;
        }

        static machine from_json(char *&jstr);
    };

    struct list {
        std::vector<machine> machines;
        void add(const machine &m) {
            machines.push_back(m);
        }
        std::string to_json() const {
            std::string jstr = "{\"machines\": [";
            bool first = true;
            for (auto m: machines) {
                if (!first) jstr += ", ";
                jstr += m.to_json();
                first = false;
            }
            jstr += "]}";
            return jstr;
        }
        
        static list from_json(char *&jstr);
    };

    std::unordered_map<std::string, any> params_from_json(char *machine_json);
}
