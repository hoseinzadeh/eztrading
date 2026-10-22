// SML (State Machine Language) support for the Monaco editor: tokenizer,
// editing rules, snippets and a dark theme. Keywords come from the grammar in
// core/transpiler/src/eztrading.pest.

function registerSml(monaco) {
    monaco.languages.register({ id: 'sml', extensions: ['.sm'], aliases: ['SML'] });

    monaco.languages.setLanguageConfiguration('sml', {
        comments: { lineComment: '//', blockComment: ['/*', '*/'] },
        brackets: [['{', '}'], ['[', ']'], ['(', ')'], ['<', '>']],
        autoClosingPairs: [
            { open: '{', close: '}' }, { open: '[', close: ']' }, { open: '(', close: ')' },
            { open: '"', close: '"', notIn: ['string', 'comment'] },
        ],
        surroundingPairs: [{ open: '{', close: '}' }, { open: '(', close: ')' }, { open: '"', close: '"' }],
        indentationRules: {
            increaseIndentPattern: /^.*\{[^}"']*$/,
            decreaseIndentPattern: /^\s*\}/,
        },
    });

    monaco.languages.setMonarchTokensProvider('sml', {
        keywords: [
            'machine', 'process', 'state', 'goto', 'on', 'init', 'launch', 'param', 'let', 'static', 'const',
            'event', 'fn', 'class', 'use', 'if', 'else', 'for', 'while', 'loop', 'in', 'return', 'emit',
            'listen', 'record', 'every', 'from', 'at', 'as', 'options', 'signal', 'ref', 'pub', 'mut',
            'builtin', 'continuously', 'enum', 'index', 'top', 'attach',
        ],
        types: [
            'int', 'int8', 'int16', 'int32', 'int64', 'uint8', 'uint16', 'uint32', 'uint64', 'float', 'bool',
            'duration', 'Symbol', 'Ticker', 'Position', 'Account', 'Vec', 'HashMap',
        ],
        constants: ['true', 'false', 'SandBox', 'Tickers'],
        units: ['ms', 'milli', 'millis', 'sec', 'secs', 'second', 'seconds', 's', 'min', 'mins', 'minute', 'minutes',
            'hr', 'hrs', 'hour', 'hours', 'day', 'days', 'week', 'weeks', 'month', 'months', 'yr', 'year', 'years'],
        tokenizer: {
            root: [
                [/\{%/, { token: 'string.cpp', next: '@cpp' }],
                [/\/\/.*$/, 'comment'],
                [/\/\*/, { token: 'comment', next: '@comment' }],
                [/"([^"\\]|\\.)*"/, 'string'],
                [/'([^'\\]|\\.)*'/, 'string'],
                [/\d+_[a-z]+/, 'number.duration'],
                [/\d+(\.\d+)?([eE][+-]?\d+)?/, 'number'],
                [/\d{2}:\d{2}(:\d{2})?/, 'number'],
                [/[A-Za-z_][A-Za-z0-9_]*/, {
                    cases: {
                        '@keywords': 'keyword',
                        '@types': 'type',
                        '@constants': 'constant',
                        '@units': 'keyword.unit',
                        '[A-Z][A-Z0-9_]*$': 'state-name',
                        '@default': 'identifier',
                    },
                }],
                [/[{}()\[\]]/, '@brackets'],
                [/->|=>|::|\.\.=?|[<>=!]=?|&&|\|\||[-+*\/%&|^?:.,;]/, 'operator'],
            ],
            comment: [
                [/[^/*]+/, 'comment'],
                [/\*\//, { token: 'comment', next: '@pop' }],
                [/[/*]/, 'comment'],
            ],
            cpp: [
                [/%\}/, { token: 'string.cpp', next: '@pop' }],
                [/[^%]+/, 'string.cpp'],
                [/%/, 'string.cpp'],
            ],
        },
    });

    const kind = monaco.languages.CompletionItemKind;
    const snippet = monaco.languages.CompletionItemInsertTextRule.InsertAsSnippet;
    const item = (label, detail, insertText) => ({ label, detail, kind: kind.Snippet, insertText, insertTextRules: snippet });
    monaco.languages.registerCompletionItemProvider('sml', {
        provideCompletionItems: () => ({
            suggestions: [
                item('machine', 'a strategy', 'machine ${1:Name} {\n\tparam ${2:equities}: Vec<Symbol> as "${3:Equities}";\n\tparam account: Account = SandBox as "Brokerage Account";\n\n\tinit { launch (${4:watch}(sym), ${5:WAIT}) for sym in ${2:equities}; }\n\n\t$0\n}'),
                item('process', 'a state machine', 'process ${1:watch}(sym: Symbol) every ${2:5 min} {\n\tlisten Tickers[sym];\n\n\tstate ${3:WAIT}: goto ${4:BUY} on ${5:Tickers[sym] < floor};\n\t$0\n}'),
                item('state', 'a state with a guarded transition', 'state ${1:NAME}: goto ${2:NEXT} on ${3:condition};'),
                item('state-action', 'a state with an action', 'state ${1:NAME}: goto ${2:NEXT} {\n\t$0\n}'),
                item('param', 'a tunable parameter', 'param ${1:name}: ${2:float} = ${3:1.0} as "${4:Label}";'),
                item('event', 'a reusable condition', 'event ${1:name}(const signal ticker: Ticker, ${2:percent}: float) {\n\t$0\n\treturn true;\n}'),
                item('buy', 'buy shares worth an amount', 'pos = account.buy(sym, ${1:invest} / Tickers[sym].close);'),
                item('sell', 'sell the whole position', 'pos = account.sell(sym, pos.shares);'),
                item('buy_limit', 'rest a buy order at a price; returns an Order', 'order = account.buy_limit(sym, ${1:shares}, ${2:price});'),
                item('sell_limit', 'rest a sell order at a price; returns an Order', 'order = account.sell_limit(sym, ${1:shares}, ${2:price});'),
                item('sell_stop', 'sell once the price falls to a level (stop loss); returns an Order', 'order = account.sell_stop(sym, ${1:shares}, ${2:price});'),
                item('buy_stop', 'buy once the price rises to a level; returns an Order', 'order = account.buy_stop(sym, ${1:shares}, ${2:price});'),
                item('cancel', 'cancel an order', 'account.cancel(${1:order});'),
                item('cancel_all', 'cancel every working order', 'account.cancel_all();'),
                item('cash', 'cash in the account', 'account.cash()'),
                item('equity', 'account equity', 'account.equity()'),
                item('is_filled', 'true once the order has completely filled', '${1:order}.is_filled()'),
                item('is_open', 'true while the order is still working', '${1:order}.is_open()'),
            ],
        }),
    });

    monaco.editor.defineTheme('ezt-dark', {
        base: 'vs-dark',
        inherit: true,
        rules: [
            { token: 'keyword', foreground: 'e8914d', fontStyle: 'bold' },
            { token: 'keyword.unit', foreground: 'd7ba7d' },
            { token: 'type', foreground: '4ec9b0' },
            { token: 'constant', foreground: '569cd6' },
            { token: 'state-name', foreground: 'c586c0' },
            { token: 'number', foreground: 'b5cea8' },
            { token: 'number.duration', foreground: 'd7ba7d' },
            { token: 'string', foreground: 'ce9178' },
            { token: 'string.cpp', foreground: '9a9a9a', fontStyle: 'italic' },
            { token: 'comment', foreground: '6a9955' },
        ],
        colors: {
            'editor.background': '#14110f',
            'editor.lineHighlightBackground': '#1d1916',
            'editorLineNumber.foreground': '#5a5048',
            'editorLineNumber.activeForeground': '#e8914d',
        },
    });
}
