// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
// Generated; regenerate from the catalog. Fluent runtime: see LICENSE.fluent.
(() => {
"use strict";
const types = (() => {
/**
 * The `FluentType` class is the base of Fluent's type system.
 *
 * Fluent types wrap JavaScript values and store additional configuration for
 * them, which can then be used in the `toString` method together with a proper
 * `Intl` formatter.
 */
class FluentType {
    /**
     * Create a `FluentType` instance.
     *
     * @param value The JavaScript value to wrap.
     */
    constructor(value) {
        this.value = value;
    }
    /**
     * Unwrap the raw value stored by this `FluentType`.
     */
    valueOf() {
        return this.value;
    }
}
/**
 * A `FluentType` representing no correct value.
 */
class FluentNone extends FluentType {
    /**
     * Create an instance of `FluentNone` with an optional fallback value.
     * @param value The fallback value of this `FluentNone`.
     */
    constructor(value = "???") {
        super(value);
    }
    /**
     * Format this `FluentNone` to the fallback string.
     */
    toString(scope) {
        return `{${this.value}}`;
    }
}
/**
 * A `FluentType` representing a number.
 *
 * A `FluentNumber` instance stores the number value of the number it
 * represents. It may also store an option bag of options which will be passed
 * to `Intl.NumerFormat` when the `FluentNumber` is formatted to a string.
 */
class FluentNumber extends FluentType {
    /**
     * Create an instance of `FluentNumber` with options to the
     * `Intl.NumberFormat` constructor.
     *
     * @param value The number value of this `FluentNumber`.
     * @param opts Options which will be passed to `Intl.NumberFormat`.
     */
    constructor(value, opts = {}) {
        super(value);
        this.opts = opts;
    }
    /**
     * Format this `FluentNumber` to a string.
     */
    toString(scope) {
        try {
            const nf = scope.memoizeIntlObject(Intl.NumberFormat, this.opts);
            return nf.format(this.value);
        }
        catch (err) {
            scope.reportError(err);
            return this.value.toString(10);
        }
    }
}
/**
 * A `FluentType` representing a date and time.
 *
 * A `FluentDateTime` instance stores the number value of the date it
 * represents, as a numerical timestamp in milliseconds. It may also store an
 * option bag of options which will be passed to `Intl.DateTimeFormat` when the
 * `FluentDateTime` is formatted to a string.
 */
class FluentDateTime extends FluentType {
    /**
     * Create an instance of `FluentDateTime` with options to the
     * `Intl.DateTimeFormat` constructor.
     *
     * @param value The number value of this `FluentDateTime`, in milliseconds.
     * @param opts Options which will be passed to `Intl.DateTimeFormat`.
     */
    constructor(value, opts = {}) {
        super(value);
        this.opts = opts;
    }
    /**
     * Format this `FluentDateTime` to a string.
     */
    toString(scope) {
        try {
            const dtf = scope.memoizeIntlObject(Intl.DateTimeFormat, this.opts);
            return dtf.format(this.value);
        }
        catch (err) {
            scope.reportError(err);
            return new Date(this.value).toISOString();
        }
    }
}
return { FluentType, FluentNone, FluentNumber, FluentDateTime };
})();
const scope = (() => {
class Scope {
    constructor(bundle, errors, args) {
        /**
         * The Set of patterns already encountered during this resolution.
         * Used to detect and prevent cyclic resolutions.
         * @ignore
         */
        this.dirty = new WeakSet();
        /** A dict of parameters passed to a TermReference. */
        this.params = null;
        /**
         * The running count of placeables resolved so far.
         * Used to detect the Billion Laughs and Quadratic Blowup attacks.
         * @ignore
         */
        this.placeables = 0;
        this.bundle = bundle;
        this.errors = errors;
        this.args = args;
    }
    reportError(error) {
        if (!this.errors || !(error instanceof Error)) {
            throw error;
        }
        this.errors.push(error);
    }
    memoizeIntlObject(ctor, opts) {
        let cache = this.bundle._intls.get(ctor);
        if (!cache) {
            cache = {};
            this.bundle._intls.set(ctor, cache);
        }
        let id = JSON.stringify(opts);
        if (!cache[id]) {
            cache[id] = new ctor(this.bundle.locales, opts);
        }
        return cache[id];
    }
}
return { Scope };
})();
const memoizer = (() => {
const cache = new Map();
function getMemoizerForLocale(locales) {
    const stringLocale = Array.isArray(locales) ? locales.join(" ") : locales;
    let memoizer = cache.get(stringLocale);
    if (memoizer === undefined) {
        memoizer = new Map();
        cache.set(stringLocale, memoizer);
    }
    return memoizer;
}
return { getMemoizerForLocale };
})();
const resolver = (() => {
/* global Intl */
/**
 * @overview
 *
 * The role of the Fluent resolver is to format a `Pattern` to an instance of
 * `FluentValue`. For performance reasons, primitive strings are considered
 * such instances, too.
 *
 * Translations can contain references to other messages or variables,
 * conditional logic in form of select expressions, traits which describe their
 * grammatical features, and can use Fluent builtins which make use of the
 * `Intl` formatters to format numbers and dates into the bundle's languages.
 * See the documentation of the Fluent syntax for more information.
 *
 * In case of errors the resolver will try to salvage as much of the
 * translation as possible. In rare situations where the resolver didn't know
 * how to recover from an error it will return an instance of `FluentNone`.
 *
 * All expressions resolve to an instance of `FluentValue`. The caller should
 * use the `toString` method to convert the instance to a native value.
 *
 * Functions in this file pass around an instance of the `Scope` class, which
 * stores the data required for successful resolution and error recovery.
 */
const { FluentType, FluentNone, FluentNumber, FluentDateTime, } = types;
/**
 * The maximum number of placeables which can be expanded in a single call to
 * `formatPattern`. The limit protects against the Billion Laughs and Quadratic
 * Blowup attacks.
 */
const MAX_PLACEABLES = 100;
/** Unicode bidi isolation characters. */
const FSI = "\u2068";
const PDI = "\u2069";
/** Helper: match a variant key to the given selector. */
function match(scope, selector, key) {
    if (key === selector) {
        // Both are strings.
        return true;
    }
    // XXX Consider comparing options too, e.g. minimumFractionDigits.
    if (key instanceof FluentNumber &&
        selector instanceof FluentNumber &&
        key.value === selector.value) {
        return true;
    }
    if (selector instanceof FluentNumber && typeof key === "string") {
        let category = scope
            .memoizeIntlObject(Intl.PluralRules, selector.opts)
            .select(selector.value);
        if (key === category) {
            return true;
        }
    }
    return false;
}
/** Helper: resolve the default variant from a list of variants. */
function getDefault(scope, variants, star) {
    if (variants[star]) {
        return resolvePattern(scope, variants[star].value);
    }
    scope.reportError(new RangeError("No default"));
    return new FluentNone();
}
/** Helper: resolve arguments to a call expression. */
function getArguments(scope, args) {
    const positional = [];
    const named = Object.create(null);
    for (const arg of args) {
        if (arg.type === "narg") {
            named[arg.name] = resolveExpression(scope, arg.value);
        }
        else {
            positional.push(resolveExpression(scope, arg));
        }
    }
    return { positional, named };
}
/** Resolve an expression to a Fluent type. */
function resolveExpression(scope, expr) {
    switch (expr.type) {
        case "str":
            return expr.value;
        case "num":
            return new FluentNumber(expr.value, {
                minimumFractionDigits: expr.precision,
            });
        case "var":
            return resolveVariableReference(scope, expr);
        case "mesg":
            return resolveMessageReference(scope, expr);
        case "term":
            return resolveTermReference(scope, expr);
        case "func":
            return resolveFunctionReference(scope, expr);
        case "select":
            return resolveSelectExpression(scope, expr);
        default:
            return new FluentNone();
    }
}
/** Resolve a reference to a variable. */
function resolveVariableReference(scope, { name }) {
    let arg;
    if (scope.params) {
        // We're inside a TermReference. It's OK to reference undefined parameters.
        if (Object.prototype.hasOwnProperty.call(scope.params, name)) {
            arg = scope.params[name];
        }
        else {
            return new FluentNone(`$${name}`);
        }
    }
    else if (scope.args &&
        Object.prototype.hasOwnProperty.call(scope.args, name)) {
        // We're in the top-level Pattern or inside a MessageReference. Missing
        // variables references produce ReferenceErrors.
        arg = scope.args[name];
    }
    else {
        scope.reportError(new ReferenceError(`Unknown variable: $${name}`));
        return new FluentNone(`$${name}`);
    }
    // Return early if the argument already is an instance of FluentType.
    if (arg instanceof FluentType) {
        return arg;
    }
    // Convert the argument to a Fluent type.
    switch (typeof arg) {
        case "string":
            return arg;
        case "number":
            return new FluentNumber(arg);
        case "object":
            if (arg instanceof Date) {
                return new FluentDateTime(arg.getTime());
            }
        // eslint-disable-next-line no-fallthrough
        default:
            scope.reportError(new TypeError(`Variable type not supported: $${name}, ${typeof arg}`));
            return new FluentNone(`$${name}`);
    }
}
/** Resolve a reference to another message. */
function resolveMessageReference(scope, { name, attr }) {
    const message = scope.bundle._messages.get(name);
    if (!message) {
        scope.reportError(new ReferenceError(`Unknown message: ${name}`));
        return new FluentNone(name);
    }
    if (attr) {
        const attribute = message.attributes[attr];
        if (attribute) {
            return resolvePattern(scope, attribute);
        }
        scope.reportError(new ReferenceError(`Unknown attribute: ${attr}`));
        return new FluentNone(`${name}.${attr}`);
    }
    if (message.value) {
        return resolvePattern(scope, message.value);
    }
    scope.reportError(new ReferenceError(`No value: ${name}`));
    return new FluentNone(name);
}
/** Resolve a call to a Term with key-value arguments. */
function resolveTermReference(scope, { name, attr, args }) {
    const id = `-${name}`;
    const term = scope.bundle._terms.get(id);
    if (!term) {
        scope.reportError(new ReferenceError(`Unknown term: ${id}`));
        return new FluentNone(id);
    }
    if (attr) {
        const attribute = term.attributes[attr];
        if (attribute) {
            // Every TermReference has its own variables.
            scope.params = getArguments(scope, args).named;
            const resolved = resolvePattern(scope, attribute);
            scope.params = null;
            return resolved;
        }
        scope.reportError(new ReferenceError(`Unknown attribute: ${attr}`));
        return new FluentNone(`${id}.${attr}`);
    }
    scope.params = getArguments(scope, args).named;
    const resolved = resolvePattern(scope, term.value);
    scope.params = null;
    return resolved;
}
/** Resolve a call to a Function with positional and key-value arguments. */
function resolveFunctionReference(scope, { name, args }) {
    // Some functions are built-in. Others may be provided by the runtime via
    // the `FluentBundle` constructor.
    let func = scope.bundle._functions[name];
    if (!func) {
        scope.reportError(new ReferenceError(`Unknown function: ${name}()`));
        return new FluentNone(`${name}()`);
    }
    if (typeof func !== "function") {
        scope.reportError(new TypeError(`Function ${name}() is not callable`));
        return new FluentNone(`${name}()`);
    }
    try {
        let resolved = getArguments(scope, args);
        return func(resolved.positional, resolved.named);
    }
    catch (err) {
        scope.reportError(err);
        return new FluentNone(`${name}()`);
    }
}
/** Resolve a select expression to the member object. */
function resolveSelectExpression(scope, { selector, variants, star }) {
    let sel = resolveExpression(scope, selector);
    if (sel instanceof FluentNone) {
        return getDefault(scope, variants, star);
    }
    // Match the selector against keys of each variant, in order.
    for (const variant of variants) {
        const key = resolveExpression(scope, variant.key);
        if (match(scope, sel, key)) {
            return resolvePattern(scope, variant.value);
        }
    }
    return getDefault(scope, variants, star);
}
/** Resolve a pattern (a complex string with placeables). */
function resolveComplexPattern(scope, ptn) {
    if (scope.dirty.has(ptn)) {
        scope.reportError(new RangeError("Cyclic reference"));
        return new FluentNone();
    }
    // Tag the pattern as dirty for the purpose of the current resolution.
    scope.dirty.add(ptn);
    const result = [];
    // Wrap interpolations with Directional Isolate Formatting characters
    // only when the pattern has more than one element.
    const useIsolating = scope.bundle._useIsolating && ptn.length > 1;
    for (const elem of ptn) {
        if (typeof elem === "string") {
            result.push(scope.bundle._transform(elem));
            continue;
        }
        scope.placeables++;
        if (scope.placeables > MAX_PLACEABLES) {
            scope.dirty.delete(ptn);
            // This is a fatal error which causes the resolver to instantly bail out
            // on this pattern. The length check protects against excessive memory
            // usage, and throwing protects against eating up the CPU when long
            // placeables are deeply nested.
            throw new RangeError(`Too many placeables expanded: ${scope.placeables}, ` +
                `max allowed is ${MAX_PLACEABLES}`);
        }
        if (useIsolating) {
            result.push(FSI);
        }
        result.push(resolveExpression(scope, elem).toString(scope));
        if (useIsolating) {
            result.push(PDI);
        }
    }
    scope.dirty.delete(ptn);
    return result.join("");
}
/**
 * Resolve a simple or a complex Pattern to a FluentString
 * (which is really the string primitive).
 */
function resolvePattern(scope, value) {
    // Resolve a simple pattern.
    if (typeof value === "string") {
        return scope.bundle._transform(value);
    }
    return resolveComplexPattern(scope, value);
}
return { resolveComplexPattern };
})();
const builtins = (() => {
/**
 * @overview
 *
 * The FTL resolver ships with a number of functions built-in.
 *
 * Each function take two arguments:
 *   - args - an array of positional args
 *   - opts - an object of key-value args
 *
 * Arguments to functions are guaranteed to already be instances of
 * `FluentValue`.  Functions must return `FluentValues` as well.
 */
const { FluentNone, FluentNumber, FluentDateTime, } = types;
function values(opts, allowed) {
    const unwrapped = Object.create(null);
    for (const [name, opt] of Object.entries(opts)) {
        if (allowed.includes(name)) {
            unwrapped[name] = opt.valueOf();
        }
    }
    return unwrapped;
}
const NUMBER_ALLOWED = [
    "unitDisplay",
    "currencyDisplay",
    "useGrouping",
    "minimumIntegerDigits",
    "minimumFractionDigits",
    "maximumFractionDigits",
    "minimumSignificantDigits",
    "maximumSignificantDigits",
];
/**
 * The implementation of the `NUMBER()` builtin available to translations.
 *
 * Translations may call the `NUMBER()` builtin in order to specify formatting
 * options of a number. For example:
 *
 *     pi = The value of \u03c0 is {NUMBER($pi, maximumFractionDigits: 2)}.
 *
 * The implementation expects an array of `FluentValues` representing the
 * positional arguments, and an object of named `FluentValues` representing the
 * named parameters.
 *
 * The following options are recognized:
 *
 *     unitDisplay
 *     currencyDisplay
 *     useGrouping
 *     minimumIntegerDigits
 *     minimumFractionDigits
 *     maximumFractionDigits
 *     minimumSignificantDigits
 *     maximumSignificantDigits
 *
 * Other options are ignored.
 *
 * @param args The positional arguments passed to this `NUMBER()`.
 * @param opts The named argments passed to this `NUMBER()`.
 */
function NUMBER(args, opts) {
    let arg = args[0];
    if (arg instanceof FluentNone) {
        return new FluentNone(`NUMBER(${arg.valueOf()})`);
    }
    if (arg instanceof FluentNumber) {
        return new FluentNumber(arg.valueOf(), {
            ...arg.opts,
            ...values(opts, NUMBER_ALLOWED),
        });
    }
    if (arg instanceof FluentDateTime) {
        return new FluentNumber(arg.valueOf(), {
            ...values(opts, NUMBER_ALLOWED),
        });
    }
    throw new TypeError("Invalid argument to NUMBER");
}
const DATETIME_ALLOWED = [
    "dateStyle",
    "timeStyle",
    "fractionalSecondDigits",
    "dayPeriod",
    "hour12",
    "weekday",
    "era",
    "year",
    "month",
    "day",
    "hour",
    "minute",
    "second",
    "timeZoneName",
];
/**
 * The implementation of the `DATETIME()` builtin available to translations.
 *
 * Translations may call the `DATETIME()` builtin in order to specify
 * formatting options of a number. For example:
 *
 *     now = It's {DATETIME($today, month: "long")}.
 *
 * The implementation expects an array of `FluentValues` representing the
 * positional arguments, and an object of named `FluentValues` representing the
 * named parameters.
 *
 * The following options are recognized:
 *
 *     dateStyle
 *     timeStyle
 *     fractionalSecondDigits
 *     dayPeriod
 *     hour12
 *     weekday
 *     era
 *     year
 *     month
 *     day
 *     hour
 *     minute
 *     second
 *     timeZoneName
 *
 * Other options are ignored.
 *
 * @param args The positional arguments passed to this `DATETIME()`.
 * @param opts The named argments passed to this `DATETIME()`.
 */
function DATETIME(args, opts) {
    let arg = args[0];
    if (arg instanceof FluentNone) {
        return new FluentNone(`DATETIME(${arg.valueOf()})`);
    }
    if (arg instanceof FluentDateTime) {
        return new FluentDateTime(arg.valueOf(), {
            ...arg.opts,
            ...values(opts, DATETIME_ALLOWED),
        });
    }
    if (arg instanceof FluentNumber) {
        return new FluentDateTime(arg.valueOf(), {
            ...values(opts, DATETIME_ALLOWED),
        });
    }
    throw new TypeError("Invalid argument to DATETIME");
}
return { NUMBER, DATETIME };
})();
const bundle = (() => {
const { resolveComplexPattern } = resolver;
const { Scope } = scope;
const { FluentNone } = types;
const { NUMBER, DATETIME } = builtins;
const { getMemoizerForLocale } = memoizer;
/**
 * Message bundles are single-language stores of translation resources. They are
 * responsible for formatting message values and attributes to strings.
 */
class FluentBundle {
    /**
     * Create an instance of `FluentBundle`.
     *
     * @example
     * ```js
     * let bundle = new FluentBundle(["en-US", "en"]);
     *
     * let bundle = new FluentBundle(locales, {useIsolating: false});
     *
     * let bundle = new FluentBundle(locales, {
     *   useIsolating: true,
     *   functions: {
     *     NODE_ENV: () => process.env.NODE_ENV
     *   }
     * });
     * ```
     *
     * @param locales - Used to instantiate `Intl` formatters used by translations.
     * @param options - Optional configuration for the bundle.
     */
    constructor(locales, { functions, useIsolating = true, transform = (v) => v, } = {}) {
        /** @ignore */
        this._terms = new Map();
        /** @ignore */
        this._messages = new Map();
        this.locales = Array.isArray(locales) ? locales : [locales];
        this._functions = {
            NUMBER,
            DATETIME,
            ...functions,
        };
        this._useIsolating = useIsolating;
        this._transform = transform;
        this._intls = getMemoizerForLocale(locales);
    }
    /**
     * Check if a message is present in the bundle.
     *
     * @param id - The identifier of the message to check.
     */
    hasMessage(id) {
        return this._messages.has(id);
    }
    /**
     * Return a raw unformatted message object from the bundle.
     *
     * Raw messages are `{value, attributes}` shapes containing translation units
     * called `Patterns`. `Patterns` are implementation-specific; they should be
     * treated as black boxes and formatted with `FluentBundle.formatPattern`.
     *
     * @param id - The identifier of the message to check.
     */
    getMessage(id) {
        return this._messages.get(id);
    }
    /**
     * Add a translation resource to the bundle.
     *
     * @example
     * ```js
     * let res = new FluentResource("foo = Foo");
     * bundle.addResource(res);
     * bundle.getMessage("foo");
     * // \u2192 {value: .., attributes: {..}}
     * ```
     *
     * @param res
     * @param options
     */
    addResource(res, { allowOverrides = false, } = {}) {
        const errors = [];
        for (let i = 0; i < res.body.length; i++) {
            let entry = res.body[i];
            if (entry.id.startsWith("-")) {
                // Identifiers starting with a dash (-) define terms. Terms are private
                // and cannot be retrieved from FluentBundle.
                if (allowOverrides === false && this._terms.has(entry.id)) {
                    errors.push(new Error(`Attempt to override an existing term: "${entry.id}"`));
                    continue;
                }
                this._terms.set(entry.id, entry);
            }
            else {
                if (allowOverrides === false && this._messages.has(entry.id)) {
                    errors.push(new Error(`Attempt to override an existing message: "${entry.id}"`));
                    continue;
                }
                this._messages.set(entry.id, entry);
            }
        }
        return errors;
    }
    /**
     * Format a `Pattern` to a string.
     *
     * Format a raw `Pattern` into a string. `args` will be used to resolve
     * references to variables passed as arguments to the translation.
     *
     * In case of errors `formatPattern` will try to salvage as much of the
     * translation as possible and will still return a string. For performance
     * reasons, the encountered errors are not returned but instead are appended
     * to the `errors` array passed as the third argument.
     *
     * If `errors` is omitted, the first encountered error will be thrown.
     *
     * @example
     * ```js
     * let errors = [];
     * bundle.addResource(
     *     new FluentResource("hello = Hello, {$name}!"));
     *
     * let hello = bundle.getMessage("hello");
     * if (hello.value) {
     *     bundle.formatPattern(hello.value, {name: "Jane"}, errors);
     *     // Returns "Hello, Jane!" and `errors` is empty.
     *
     *     bundle.formatPattern(hello.value, undefined, errors);
     *     // Returns "Hello, {$name}!" and `errors` is now:
     *     // [<ReferenceError: Unknown variable: name>]
     * }
     * ```
     */
    formatPattern(pattern, args = null, errors = null) {
        // Resolve a simple pattern without creating a scope. No error handling is
        // required; by definition simple patterns don't have placeables.
        if (typeof pattern === "string") {
            return this._transform(pattern);
        }
        // Resolve a complex pattern.
        let scope = new Scope(this, errors, args);
        try {
            let value = resolveComplexPattern(scope, pattern);
            return value.toString(scope);
        }
        catch (err) {
            if (scope.errors && err instanceof Error) {
                scope.errors.push(err);
                return new FluentNone().toString(scope);
            }
            throw err;
        }
    }
}
return { FluentBundle };
})();
const resource = (() => {
// This regex is used to iterate through the beginnings of messages and terms.
// With the /m flag, the ^ matches at the beginning of every line.
const RE_MESSAGE_START = /^(-?[a-zA-Z][\w-]*) *= */gm;
// Both Attributes and Variants are parsed in while loops. These regexes are
// used to break out of them.
const RE_ATTRIBUTE_START = /\.([a-zA-Z][\w-]*) *= */y;
const RE_VARIANT_START = /\*?\[/y;
const RE_NUMBER_LITERAL = /(-?[0-9]+(?:\.([0-9]+))?)/y;
const RE_IDENTIFIER = /([a-zA-Z][\w-]*)/y;
const RE_REFERENCE = /([$-])?([a-zA-Z][\w-]*)(?:\.([a-zA-Z][\w-]*))?/y;
const RE_FUNCTION_NAME = /^[A-Z][A-Z0-9_-]*$/;
// A "run" is a sequence of text or string literal characters which don't
// require any special handling. For TextElements such special characters are: {
// (starts a placeable), and line breaks which require additional logic to check
// if the next line is indented. For StringLiterals they are: \ (starts an
// escape sequence), " (ends the literal), and line breaks which are not allowed
// in StringLiterals. Note that string runs may be empty; text runs may not.
const RE_TEXT_RUN = /([^{}\n\r]+)/y;
const RE_STRING_RUN = /([^\\"\n\r]*)/y;
// Escape sequences.
const RE_STRING_ESCAPE = /\\([\\"])/y;
const RE_UNICODE_ESCAPE = /\\u([a-fA-F0-9]{4})|\\U([a-fA-F0-9]{6})/y;
// Used for trimming TextElements and indents.
const RE_LEADING_NEWLINES = /^\n+/;
const RE_TRAILING_SPACES = / +$/;
// Used in makeIndent to strip spaces from blank lines and normalize CRLF to LF.
const RE_BLANK_LINES = / *\r?\n/g;
// Used in makeIndent to measure the indentation.
const RE_INDENT = /( *)$/;
// Common tokens.
const TOKEN_BRACE_OPEN = /{\s*/y;
const TOKEN_BRACE_CLOSE = /\s*}/y;
const TOKEN_BRACKET_OPEN = /\[\s*/y;
const TOKEN_BRACKET_CLOSE = /\s*] */y;
const TOKEN_PAREN_OPEN = /\s*\(\s*/y;
const TOKEN_ARROW = /\s*->\s*/y;
const TOKEN_COLON = /\s*:\s*/y;
// Note the optional comma. As a deviation from the Fluent EBNF, the parser
// doesn't enforce commas between call arguments.
const TOKEN_COMMA = /\s*,?\s*/y;
const TOKEN_BLANK = /\s+/y;
/**
 * Fluent Resource is a structure storing parsed localization entries.
 */
class FluentResource {
    constructor(source) {
        this.body = [];
        RE_MESSAGE_START.lastIndex = 0;
        let cursor = 0;
        // Iterate over the beginnings of messages and terms to efficiently skip
        // comments and recover from errors.
        while (true) {
            let next = RE_MESSAGE_START.exec(source);
            if (next === null) {
                break;
            }
            cursor = RE_MESSAGE_START.lastIndex;
            try {
                this.body.push(parseMessage(next[1]));
            }
            catch (err) {
                if (err instanceof SyntaxError) {
                    // Don't report any Fluent syntax errors. Skip directly to the
                    // beginning of the next message or term.
                    continue;
                }
                throw err;
            }
        }
        // The parser implementation is inlined below for performance reasons,
        // as well as for convenience of accessing `source` and `cursor`.
        // The parser focuses on minimizing the number of false negatives at the
        // expense of increasing the risk of false positives. In other words, it
        // aims at parsing valid Fluent messages with a success rate of 100%, but it
        // may also parse a few invalid messages which the reference parser would
        // reject. The parser doesn't perform any validation and may produce entries
        // which wouldn't make sense in the real world. For best results users are
        // advised to validate translations with the fluent-syntax parser
        // pre-runtime.
        // The parser makes an extensive use of sticky regexes which can be anchored
        // to any offset of the source string without slicing it. Errors are thrown
        // to bail out of parsing of ill-formed messages.
        function test(re) {
            re.lastIndex = cursor;
            return re.test(source);
        }
        // Advance the cursor by the char if it matches. May be used as a predicate
        // (was the match found?) or, if errorClass is passed, as an assertion.
        function consumeChar(char, errorClass) {
            if (source[cursor] === char) {
                cursor++;
                return true;
            }
            if (errorClass) {
                throw new errorClass(`Expected ${char}`);
            }
            return false;
        }
        // Advance the cursor by the token if it matches. May be used as a predicate
        // (was the match found?) or, if errorClass is passed, as an assertion.
        function consumeToken(re, errorClass) {
            if (test(re)) {
                cursor = re.lastIndex;
                return true;
            }
            if (errorClass) {
                throw new errorClass(`Expected ${re.toString()}`);
            }
            return false;
        }
        // Execute a regex, advance the cursor, and return all capture groups.
        function match(re) {
            re.lastIndex = cursor;
            let result = re.exec(source);
            if (result === null) {
                throw new SyntaxError(`Expected ${re.toString()}`);
            }
            cursor = re.lastIndex;
            return result;
        }
        // Execute a regex, advance the cursor, and return the capture group.
        function match1(re) {
            return match(re)[1];
        }
        function parseMessage(id) {
            let value = parsePattern();
            let attributes = parseAttributes();
            if (value === null && Object.keys(attributes).length === 0) {
                throw new SyntaxError("Expected message value or attributes");
            }
            return { id, value, attributes };
        }
        function parseAttributes() {
            let attrs = Object.create(null);
            while (test(RE_ATTRIBUTE_START)) {
                let name = match1(RE_ATTRIBUTE_START);
                let value = parsePattern();
                if (value === null) {
                    throw new SyntaxError("Expected attribute value");
                }
                attrs[name] = value;
            }
            return attrs;
        }
        function parsePattern() {
            let first;
            // First try to parse any simple text on the same line as the id.
            if (test(RE_TEXT_RUN)) {
                first = match1(RE_TEXT_RUN);
            }
            // If there's a placeable on the first line, parse a complex pattern.
            if (source[cursor] === "{" || source[cursor] === "}") {
                // Re-use the text parsed above, if possible.
                return parsePatternElements(first ? [first] : [], Infinity);
            }
            // RE_TEXT_VALUE stops at newlines. Only continue parsing the pattern if
            // what comes after the newline is indented.
            let indent = parseIndent();
            if (indent) {
                if (first) {
                    // If there's text on the first line, the blank block is part of the
                    // translation content in its entirety.
                    return parsePatternElements([first, indent], indent.length);
                }
                // Otherwise, we're dealing with a block pattern, i.e. a pattern which
                // starts on a new line. Discrad the leading newlines but keep the
                // inline indent; it will be used by the dedentation logic.
                indent.value = trim(indent.value, RE_LEADING_NEWLINES);
                return parsePatternElements([indent], indent.length);
            }
            if (first) {
                // It was just a simple inline text after all.
                return trim(first, RE_TRAILING_SPACES);
            }
            return null;
        }
        // Parse a complex pattern as an array of elements.
        function parsePatternElements(elements = [], commonIndent) {
            while (true) {
                if (test(RE_TEXT_RUN)) {
                    elements.push(match1(RE_TEXT_RUN));
                    continue;
                }
                if (source[cursor] === "{") {
                    elements.push(parsePlaceable());
                    continue;
                }
                if (source[cursor] === "}") {
                    throw new SyntaxError("Unbalanced closing brace");
                }
                let indent = parseIndent();
                if (indent) {
                    elements.push(indent);
                    commonIndent = Math.min(commonIndent, indent.length);
                    continue;
                }
                break;
            }
            let lastIndex = elements.length - 1;
            let lastElement = elements[lastIndex];
            // Trim the trailing spaces in the last element if it's a TextElement.
            if (typeof lastElement === "string") {
                elements[lastIndex] = trim(lastElement, RE_TRAILING_SPACES);
            }
            let baked = [];
            for (let element of elements) {
                if (element instanceof Indent) {
                    // Dedent indented lines by the maximum common indent.
                    element = element.value.slice(0, element.value.length - commonIndent);
                }
                if (element) {
                    baked.push(element);
                }
            }
            return baked;
        }
        function parsePlaceable() {
            consumeToken(TOKEN_BRACE_OPEN, SyntaxError);
            let selector = parseInlineExpression();
            if (consumeToken(TOKEN_BRACE_CLOSE)) {
                return selector;
            }
            if (consumeToken(TOKEN_ARROW)) {
                let variants = parseVariants();
                consumeToken(TOKEN_BRACE_CLOSE, SyntaxError);
                return {
                    type: "select",
                    selector,
                    ...variants,
                };
            }
            throw new SyntaxError("Unclosed placeable");
        }
        function parseInlineExpression() {
            if (source[cursor] === "{") {
                // It's a nested placeable.
                return parsePlaceable();
            }
            if (test(RE_REFERENCE)) {
                let [, sigil, name, attr = null] = match(RE_REFERENCE);
                if (sigil === "$") {
                    return { type: "var", name };
                }
                if (consumeToken(TOKEN_PAREN_OPEN)) {
                    let args = parseArguments();
                    if (sigil === "-") {
                        // A parameterized term: -term(...).
                        return { type: "term", name, attr, args };
                    }
                    if (RE_FUNCTION_NAME.test(name)) {
                        return { type: "func", name, args };
                    }
                    throw new SyntaxError("Function names must be all upper-case");
                }
                if (sigil === "-") {
                    // A non-parameterized term: -term.
                    return {
                        type: "term",
                        name,
                        attr,
                        args: [],
                    };
                }
                return { type: "mesg", name, attr };
            }
            return parseLiteral();
        }
        function parseArguments() {
            let args = [];
            while (true) {
                switch (source[cursor]) {
                    case ")": // End of the argument list.
                        cursor++;
                        return args;
                    case undefined: // EOF
                        throw new SyntaxError("Unclosed argument list");
                }
                args.push(parseArgument());
                // Commas between arguments are treated as whitespace.
                consumeToken(TOKEN_COMMA);
            }
        }
        function parseArgument() {
            let expr = parseInlineExpression();
            if (expr.type !== "mesg") {
                return expr;
            }
            if (consumeToken(TOKEN_COLON)) {
                // The reference is the beginning of a named argument.
                return {
                    type: "narg",
                    name: expr.name,
                    value: parseLiteral(),
                };
            }
            // It's a regular message reference.
            return expr;
        }
        function parseVariants() {
            let variants = [];
            let count = 0;
            let star;
            while (test(RE_VARIANT_START)) {
                if (consumeChar("*")) {
                    star = count;
                }
                let key = parseVariantKey();
                let value = parsePattern();
                if (value === null) {
                    throw new SyntaxError("Expected variant value");
                }
                variants[count++] = { key, value };
            }
            if (count === 0) {
                return null;
            }
            if (star === undefined) {
                throw new SyntaxError("Expected default variant");
            }
            return { variants, star };
        }
        function parseVariantKey() {
            consumeToken(TOKEN_BRACKET_OPEN, SyntaxError);
            let key;
            if (test(RE_NUMBER_LITERAL)) {
                key = parseNumberLiteral();
            }
            else {
                key = {
                    type: "str",
                    value: match1(RE_IDENTIFIER),
                };
            }
            consumeToken(TOKEN_BRACKET_CLOSE, SyntaxError);
            return key;
        }
        function parseLiteral() {
            if (test(RE_NUMBER_LITERAL)) {
                return parseNumberLiteral();
            }
            if (source[cursor] === '"') {
                return parseStringLiteral();
            }
            throw new SyntaxError("Invalid expression");
        }
        function parseNumberLiteral() {
            let [, value, fraction = ""] = match(RE_NUMBER_LITERAL);
            let precision = fraction.length;
            return {
                type: "num",
                value: parseFloat(value),
                precision,
            };
        }
        function parseStringLiteral() {
            consumeChar('"', SyntaxError);
            let value = "";
            while (true) {
                value += match1(RE_STRING_RUN);
                if (source[cursor] === "\\") {
                    value += parseEscapeSequence();
                    continue;
                }
                if (consumeChar('"')) {
                    return { type: "str", value };
                }
                // We've reached an EOL of EOF.
                throw new SyntaxError("Unclosed string literal");
            }
        }
        // Unescape known escape sequences.
        function parseEscapeSequence() {
            if (test(RE_STRING_ESCAPE)) {
                return match1(RE_STRING_ESCAPE);
            }
            if (test(RE_UNICODE_ESCAPE)) {
                let [, codepoint4, codepoint6] = match(RE_UNICODE_ESCAPE);
                let codepoint = parseInt(codepoint4 || codepoint6, 16);
                return codepoint <= 0xd7ff || 0xe000 <= codepoint
                    ? // It's a Unicode scalar value.
                        String.fromCodePoint(codepoint)
                    : // Lonely surrogates can cause trouble when the parsing result is
                        // saved using UTF-8. Use U+FFFD REPLACEMENT CHARACTER instead.
                        "\ufffd";
            }
            throw new SyntaxError("Unknown escape sequence");
        }
        // Parse blank space. Return it if it looks like indent before a pattern
        // line. Skip it othwerwise.
        function parseIndent() {
            let start = cursor;
            consumeToken(TOKEN_BLANK);
            // Check the first non-blank character after the indent.
            switch (source[cursor]) {
                case ".":
                case "[":
                case "*":
                case "}":
                case undefined: // EOF
                    // A special character. End the Pattern.
                    return false;
                case "{":
                    // Placeables don't require indentation (in EBNF: block-placeable).
                    // Continue the Pattern.
                    return makeIndent(source.slice(start, cursor));
            }
            // If the first character on the line is not one of the special characters
            // listed above, it's a regular text character. Check if there's at least
            // one space of indent before it.
            if (source[cursor - 1] === " ") {
                // It's an indented text character (in EBNF: indented-char). Continue
                // the Pattern.
                return makeIndent(source.slice(start, cursor));
            }
            // A not-indented text character is likely the identifier of the next
            // message. End the Pattern.
            return false;
        }
        // Trim blanks in text according to the given regex.
        function trim(text, re) {
            return text.replace(re, "");
        }
        // Normalize a blank block and extract the indent details.
        function makeIndent(blank) {
            let value = blank.replace(RE_BLANK_LINES, "\n");
            // eslint-disable-next-line @typescript-eslint/no-non-null-assertion
            let length = RE_INDENT.exec(blank)[1].length;
            return new Indent(value, length);
        }
    }
}
class Indent {
    constructor(value, length) {
        this.value = value;
        this.length = length;
    }
}
return { FluentResource };
})();
const { FluentBundle } = bundle;
const { FluentResource } = resource;
const CATALOGS = {
"en-US": "m6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e616374696f6e2e636c656172 = clear\n\nm6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e616374696f6e2e636c6f7365 = close\n\nm6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e616374696f6e2e73656e64 = send\n\nm6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e616374696f6e2e73656e642e616c6c = send to support\n\nm6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e616374696f6e2e73686f772e64657461696c73 = show details\n\nm6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e64657461696c2e6c6f636174696f6e2e6c6162656c = location\n\nm6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e64657461696c2e6d6573736167652e6c6162656c = message\n\nm6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e64657461696c2e737461636b2e6c6162656c = stack\n\nm6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e656d707479 = no system messages this session.\n\nm6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e68656164696e67 = system messages\n\nm6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e6c696e6b2e6c6162656c = system messages\n\nm6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e7265706f7274696e672e6f6666 = sending reports is off.\n\nm6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e736e617073686f742e68656164696e67 = snapshot of { $count } system messages\n\nm6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e73756d6d6172792e6c6f61642e6661696c6564 = couldn't load part of this page.\n\nm6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e73756d6d6172792e726571756573742e6661696c6564 = couldn't finish that request.\n\nm6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e73756d6d6172792e756e6578706563746564 = something on this page didn't work.\n\nm6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e7461622e616c6c = all\n\nm6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e7461622e6572726f7273 = errors\n\nm6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e7461622e696e666f = info\n\nm6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e7461622e7761726e696e6773 = warnings\n\nm6a6f75726e616c2e77696e646f772e6368726f6d652e6461792e7a6f6e65732e616c6c = this day's times are on { $zones }\n\nm6a6f75726e616c2e77696e646f772e6368726f6d652e6461792e7a6f6e65732e70617274 = some of this day's times are on { $zones }\n\nm6a6f75726e616c2e77696e646f772e6368726f6d652e6c6f672e726561642e6661696c6564 = that log couldn't be read. reload to try again.\n\nm6a6f75726e616c2e77696e646f772e6368726f6d652e72656c6f61642e68696e74 = reload to try again.\n\nm6a6f75726e616c2e77696e646f772e6368726f6d652e7265706f72742e616374696f6e2e63616e63656c = cancel\n\nm6a6f75726e616c2e77696e646f772e6368726f6d652e7265706f72742e616374696f6e2e73656e64 = continue to support\n\nm6a6f75726e616c2e77696e646f772e6368726f6d652e7265706f72742e6170702e6c6162656c = app\n\nm6a6f75726e616c2e77696e646f772e6368726f6d652e7265706f72742e627574746f6e2e6c6162656c = report this\n\nm6a6f75726e616c2e77696e646f772e6368726f6d652e7265706f72742e64656661756c742e7375626a656374 = convey error\n\nm6a6f75726e616c2e77696e646f772e6368726f6d652e7265706f72742e6572726f722e636f64652e6c6162656c = error code\n\nm6a6f75726e616c2e77696e646f772e6368726f6d652e7265706f72742e707269766163792e6e6f7465 = nothing leaves this journal until you send the form on the support website.\n\nm6a6f75726e616c2e77696e646f772e6368726f6d652e7265706f72742e726563656e742e6c6162656c = recent error lines\n\nm6a6f75726e616c2e77696e646f772e6368726f6d652e7265706f72742e726f7574652e6c6162656c = route\n\nm6a6f75726e616c2e77696e646f772e6368726f6d652e7265706f72742e7469746c65 = review this report\n\nm6a6f75726e616c2e77696e646f772e6368726f6d652e7265747279 = try again\n\nm6a6f75726e616c2e77696e646f772e6368726f6d652e73656374696f6e2e6572726f72 = couldn't load this section\n\nm6a6f75726e616c2e77696e646f772e6368726f6d652e7374696c6c2e776f726b696e67 = still working\u2026\n\nm6a6f75726e616c2e77696e646f772e6368726f6d652e756e6b6e6f776e2e6572726f72 = something went wrong. reload to try again.\n\nm6a6f75726e616c2e77696e646f772e6368726f6d652e7a6f6e652e74696d65 = { $label } time\n\nm6a6f75726e616c2e77696e646f772e6e6f742e666f756e642e64657461696c = The requested URL was not found on the server. If you entered the URL manually please check your spelling and try again.\n\nm6a6f75726e616c2e77696e646f772e6e6f742e666f756e642e68656164696e67 = Not Found\n\nm6a6f75726e616c2e77696e646f772e6e6f742e666f756e642e7469746c65 = 404 Not Found\n\nm6a6f75726e616c2e77696e646f772e7368656c6c2e636f6e6e656374696e67 = connecting\u2026\n\nm6a6f75726e616c2e77696e646f772e7368656c6c2e6c6173742e6d657373616765 = last message:\n\nm6a6f75726e616c2e77696e646f772e7368656c6c2e6c6f6164696e67 = loading\u2026\n\nm6a6f75726e616c2e77696e646f772e7368656c6c2e6e6f74696669636174696f6e732e656d707479 = no notifications yet\n\nm6a6f75726e616c2e77696e646f772e7368656c6c2e6e6f74696669636174696f6e732e656e61626c65 = enable browser notifications\n\nm6a6f75726e616c2e77696e646f772e7368656c6c2e6e6f74696669636174696f6e732e6c6162656c = notifications\n\nm6a6f75726e616c2e77696e646f772e7368656c6c2e6e6f74696669636174696f6e732e7175696574 = quiet notifications\n\nm6a6f75726e616c2e77696e646f772e7368656c6c2e6e6f74696669636174696f6e732e71756965742e6578706c616e6174696f6e = errors from background services that weren't shown as notifications.\n\nm6a6f75726e616c2e77696e646f772e7368656c6c2e6e6f74696669636174696f6e732e726563656e74 = recent notifications\n\nm6a6f75726e616c2e77696e646f772e7368656c6c2e736b69702e746f2e776f726b7370616365 = skip to workspace\n\nm6a6f75726e616c2e77696e646f772e7368656c6c2e7374617465 = state:\n\nm6a6f75726e616c2e77696e646f772e7368656c6c2e737461747573 = status\n\nm6a6f75726e616c2e77696e646f772e7368656c6c2e7374617475732e64657461696c73 = journal status details\n\nm6a6f75726e616c2e77696e646f772e7368656c6c2e746563686e6963616c2e64657461696c73 = technical details\n\nm6a6f75726e616c2e77696e646f772e7368656c6c2e7469746c65 = journal\n\nm6a6f75726e616c2e77696e646f772e7368656c6c2e7570646174652e6c696e6b = how to update your journal\n\nm6a6f75726e616c2e77696e646f772e7368656c6c2e757074696d65 = uptime:\n\nm6a6f75726e616c2e77696e646f772e7368656c6c2e76657273696f6e = version\n\nm6a6f75726e616c2e77696e646f772e7368656c6c2e7669657773 = views\n\n",
};
// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc


function cardinal(val) { return { type: "cardinal", value: val }; }
function integer(val) { return { type: "integer", value: val }; }

const LOCALES = {
  "en-US": { bundleLocale: "en-US", cardinal: "en" },
};

const CATEGORIES = {
  "en": (n) => (n == 1) ? "one" : "other",
};

const MESSAGES = {
  "journal.window.chrome.console.action.clear": {
    resourceKey: "m6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e616374696f6e2e636c656172",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.window.chrome.console.action.close": {
    resourceKey: "m6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e616374696f6e2e636c6f7365",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.window.chrome.console.action.send": {
    resourceKey: "m6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e616374696f6e2e73656e64",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.window.chrome.console.action.send.all": {
    resourceKey: "m6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e616374696f6e2e73656e642e616c6c",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.window.chrome.console.action.show.details": {
    resourceKey: "m6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e616374696f6e2e73686f772e64657461696c73",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.window.chrome.console.detail.location.label": {
    resourceKey: "m6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e64657461696c2e6c6f636174696f6e2e6c6162656c",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.window.chrome.console.detail.message.label": {
    resourceKey: "m6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e64657461696c2e6d6573736167652e6c6162656c",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.window.chrome.console.detail.stack.label": {
    resourceKey: "m6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e64657461696c2e737461636b2e6c6162656c",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.window.chrome.console.empty": {
    resourceKey: "m6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e656d707479",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.window.chrome.console.heading": {
    resourceKey: "m6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e68656164696e67",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.window.chrome.console.link.label": {
    resourceKey: "m6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e6c696e6b2e6c6162656c",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.window.chrome.console.reporting.off": {
    resourceKey: "m6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e7265706f7274696e672e6f6666",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.window.chrome.console.snapshot.heading": {
    resourceKey: "m6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e736e617073686f742e68656164696e67",
    formatIntent: "text",
    arguments: [
      { name: "count", type: "string", min: null, max: null, numericStyle: "ungrouped" },
    ],
  },
  "journal.window.chrome.console.summary.load.failed": {
    resourceKey: "m6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e73756d6d6172792e6c6f61642e6661696c6564",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.window.chrome.console.summary.request.failed": {
    resourceKey: "m6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e73756d6d6172792e726571756573742e6661696c6564",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.window.chrome.console.summary.unexpected": {
    resourceKey: "m6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e73756d6d6172792e756e6578706563746564",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.window.chrome.console.tab.all": {
    resourceKey: "m6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e7461622e616c6c",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.window.chrome.console.tab.errors": {
    resourceKey: "m6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e7461622e6572726f7273",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.window.chrome.console.tab.info": {
    resourceKey: "m6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e7461622e696e666f",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.window.chrome.console.tab.warnings": {
    resourceKey: "m6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e7461622e7761726e696e6773",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.window.chrome.day.zones.all": {
    resourceKey: "m6a6f75726e616c2e77696e646f772e6368726f6d652e6461792e7a6f6e65732e616c6c",
    formatIntent: "text",
    arguments: [
      { name: "zones", type: "string", min: null, max: null, numericStyle: "ungrouped" },
    ],
  },
  "journal.window.chrome.day.zones.part": {
    resourceKey: "m6a6f75726e616c2e77696e646f772e6368726f6d652e6461792e7a6f6e65732e70617274",
    formatIntent: "text",
    arguments: [
      { name: "zones", type: "string", min: null, max: null, numericStyle: "ungrouped" },
    ],
  },
  "journal.window.chrome.log.read.failed": {
    resourceKey: "m6a6f75726e616c2e77696e646f772e6368726f6d652e6c6f672e726561642e6661696c6564",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.window.chrome.reload.hint": {
    resourceKey: "m6a6f75726e616c2e77696e646f772e6368726f6d652e72656c6f61642e68696e74",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.window.chrome.report.action.cancel": {
    resourceKey: "m6a6f75726e616c2e77696e646f772e6368726f6d652e7265706f72742e616374696f6e2e63616e63656c",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.window.chrome.report.action.send": {
    resourceKey: "m6a6f75726e616c2e77696e646f772e6368726f6d652e7265706f72742e616374696f6e2e73656e64",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.window.chrome.report.app.label": {
    resourceKey: "m6a6f75726e616c2e77696e646f772e6368726f6d652e7265706f72742e6170702e6c6162656c",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.window.chrome.report.button.label": {
    resourceKey: "m6a6f75726e616c2e77696e646f772e6368726f6d652e7265706f72742e627574746f6e2e6c6162656c",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.window.chrome.report.default.subject": {
    resourceKey: "m6a6f75726e616c2e77696e646f772e6368726f6d652e7265706f72742e64656661756c742e7375626a656374",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.window.chrome.report.error.code.label": {
    resourceKey: "m6a6f75726e616c2e77696e646f772e6368726f6d652e7265706f72742e6572726f722e636f64652e6c6162656c",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.window.chrome.report.privacy.note": {
    resourceKey: "m6a6f75726e616c2e77696e646f772e6368726f6d652e7265706f72742e707269766163792e6e6f7465",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.window.chrome.report.recent.label": {
    resourceKey: "m6a6f75726e616c2e77696e646f772e6368726f6d652e7265706f72742e726563656e742e6c6162656c",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.window.chrome.report.route.label": {
    resourceKey: "m6a6f75726e616c2e77696e646f772e6368726f6d652e7265706f72742e726f7574652e6c6162656c",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.window.chrome.report.title": {
    resourceKey: "m6a6f75726e616c2e77696e646f772e6368726f6d652e7265706f72742e7469746c65",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.window.chrome.retry": {
    resourceKey: "m6a6f75726e616c2e77696e646f772e6368726f6d652e7265747279",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.window.chrome.section.error": {
    resourceKey: "m6a6f75726e616c2e77696e646f772e6368726f6d652e73656374696f6e2e6572726f72",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.window.chrome.still.working": {
    resourceKey: "m6a6f75726e616c2e77696e646f772e6368726f6d652e7374696c6c2e776f726b696e67",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.window.chrome.unknown.error": {
    resourceKey: "m6a6f75726e616c2e77696e646f772e6368726f6d652e756e6b6e6f776e2e6572726f72",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.window.chrome.zone.time": {
    resourceKey: "m6a6f75726e616c2e77696e646f772e6368726f6d652e7a6f6e652e74696d65",
    formatIntent: "text",
    arguments: [
      { name: "label", type: "string", min: null, max: null, numericStyle: "ungrouped" },
    ],
  },
  "journal.window.not.found.detail": {
    resourceKey: "m6a6f75726e616c2e77696e646f772e6e6f742e666f756e642e64657461696c",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.window.not.found.heading": {
    resourceKey: "m6a6f75726e616c2e77696e646f772e6e6f742e666f756e642e68656164696e67",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.window.not.found.title": {
    resourceKey: "m6a6f75726e616c2e77696e646f772e6e6f742e666f756e642e7469746c65",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.window.shell.connecting": {
    resourceKey: "m6a6f75726e616c2e77696e646f772e7368656c6c2e636f6e6e656374696e67",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.window.shell.last.message": {
    resourceKey: "m6a6f75726e616c2e77696e646f772e7368656c6c2e6c6173742e6d657373616765",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.window.shell.loading": {
    resourceKey: "m6a6f75726e616c2e77696e646f772e7368656c6c2e6c6f6164696e67",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.window.shell.notifications.empty": {
    resourceKey: "m6a6f75726e616c2e77696e646f772e7368656c6c2e6e6f74696669636174696f6e732e656d707479",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.window.shell.notifications.enable": {
    resourceKey: "m6a6f75726e616c2e77696e646f772e7368656c6c2e6e6f74696669636174696f6e732e656e61626c65",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.window.shell.notifications.label": {
    resourceKey: "m6a6f75726e616c2e77696e646f772e7368656c6c2e6e6f74696669636174696f6e732e6c6162656c",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.window.shell.notifications.quiet": {
    resourceKey: "m6a6f75726e616c2e77696e646f772e7368656c6c2e6e6f74696669636174696f6e732e7175696574",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.window.shell.notifications.quiet.explanation": {
    resourceKey: "m6a6f75726e616c2e77696e646f772e7368656c6c2e6e6f74696669636174696f6e732e71756965742e6578706c616e6174696f6e",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.window.shell.notifications.recent": {
    resourceKey: "m6a6f75726e616c2e77696e646f772e7368656c6c2e6e6f74696669636174696f6e732e726563656e74",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.window.shell.skip.to.workspace": {
    resourceKey: "m6a6f75726e616c2e77696e646f772e7368656c6c2e736b69702e746f2e776f726b7370616365",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.window.shell.state": {
    resourceKey: "m6a6f75726e616c2e77696e646f772e7368656c6c2e7374617465",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.window.shell.status": {
    resourceKey: "m6a6f75726e616c2e77696e646f772e7368656c6c2e737461747573",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.window.shell.status.details": {
    resourceKey: "m6a6f75726e616c2e77696e646f772e7368656c6c2e7374617475732e64657461696c73",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.window.shell.technical.details": {
    resourceKey: "m6a6f75726e616c2e77696e646f772e7368656c6c2e746563686e6963616c2e64657461696c73",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.window.shell.title": {
    resourceKey: "m6a6f75726e616c2e77696e646f772e7368656c6c2e7469746c65",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.window.shell.update.link": {
    resourceKey: "m6a6f75726e616c2e77696e646f772e7368656c6c2e7570646174652e6c696e6b",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.window.shell.uptime": {
    resourceKey: "m6a6f75726e616c2e77696e646f772e7368656c6c2e757074696d65",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.window.shell.version": {
    resourceKey: "m6a6f75726e616c2e77696e646f772e7368656c6c2e76657273696f6e",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.window.shell.views": {
    resourceKey: "m6a6f75726e616c2e77696e646f772e7368656c6c2e7669657773",
    formatIntent: "text",
    arguments: [
    ],
  },
};

const BUNDLES = new Map();

function getBundle(locale, useIsolating) {
  const cacheKey = `${locale}:${useIsolating}`;
  let bundle = BUNDLES.get(cacheKey);
  if (!bundle) {
    const locInfo = LOCALES[locale];
    if (!locInfo) throw new Error(`Unknown locale: ${locale}`);
    const ftlSource = CATALOGS[locale];
    if (ftlSource === undefined) throw new Error(`Unknown catalog: ${locale}`);
    const resource = new FluentResource(ftlSource);
    bundle = new FluentBundle(locInfo.bundleLocale, { useIsolating });
    const errors = bundle.addResource(resource);
    if (errors.length > 0) throw new Error(`FTL errors: ${errors.join(", ")}`);
    BUNDLES.set(cacheKey, bundle);
  }
  return bundle;
}

function render(locale, messageId, args = [], { useIsolating = true } = {}) {
  if (typeof useIsolating !== "boolean") throw new Error("Expected boolean useIsolating");
  const locInfo = LOCALES[locale];
  if (!locInfo) throw new Error(`Unknown locale: ${locale}`);

  const msgInfo = MESSAGES[messageId];
  if (!msgInfo) throw new Error(`Unknown message: ${messageId}`);

  const seen = new Set();
  for (const [k] of args) {
    if (seen.has(k)) throw new Error(`Duplicate argument: ${k}`);
    seen.add(k);
  }

  const declaredMap = new Map(msgInfo.arguments.map(a => [a.name, a]));
  for (const [k] of args) {
    if (!declaredMap.has(k)) throw new Error(`Unknown argument: ${k}`);
  }
  for (const k of declaredMap.keys()) {
    if (!seen.has(k)) throw new Error(`Missing argument: ${k}`);
  }

  const fArgs = {};
  let selectedVariant = msgInfo.enumDefault || null;

  for (const [k, v] of args) {
    const spec = declaredMap.get(k);
    if (spec.type === "string") {
      if (typeof v !== "string") throw new Error(`Expected string for ${k}`);
      if (msgInfo.formatIntent === "enum" && msgInfo.selectorVar === k) {
        if (msgInfo.enumValues.includes(v)) {
          selectedVariant = v;
        } else {
          selectedVariant = msgInfo.enumDefault;
        }
      }
      fArgs[k] = v;
    } else if (spec.type === "boolean") {
      if (typeof v !== "boolean") throw new Error(`Expected boolean for ${k}`);
      fArgs[k] = v ? "true" : "false";
    } else if (spec.type === "integer") {
      if (v && typeof v === "object" && v.type === "cardinal") throw new Error(`Expected integer, got cardinal for ${k}`);
      const num = (v && typeof v === "object" && v.type === "integer") ? v.value : v;
      if (typeof num !== "number" && typeof num !== "bigint") throw new Error(`Expected number for ${k}`);
      if (typeof num === "number" && (!Number.isFinite(num) || !Number.isInteger(num))) throw new Error(`Expected integer number for ${k}`);
      const nVal = BigInt(num);
      const minVal = spec.min !== null ? BigInt(spec.min) : -9007199254740991n;
      const maxVal = spec.max !== null ? BigInt(spec.max) : 9007199254740991n;
      const effMin = minVal < -9007199254740991n ? -9007199254740991n : minVal;
      const effMax = maxVal > 9007199254740991n ? 9007199254740991n : maxVal;
      if (nVal < effMin || nVal > effMax) throw new Error(`Out of range for ${k}`);
      fArgs[k] = Number(nVal);
    } else if (spec.type === "cardinal") {
      if (v && typeof v === "object" && v.type === "integer") throw new Error(`Expected cardinal, got integer for ${k}`);
      const num = (v && typeof v === "object" && v.type === "cardinal") ? v.value : v;
      if (typeof num !== "number" && typeof num !== "bigint") throw new Error(`Expected number for ${k}`);
      if (typeof num === "number" && (!Number.isFinite(num) || !Number.isInteger(num))) throw new Error(`Expected integer number for ${k}`);
      const nVal = BigInt(num);
      const minVal = spec.min !== null ? BigInt(spec.min) : -2147483648n;
      const maxVal = spec.max !== null ? BigInt(spec.max) : 2147483647n;
      const effMin = minVal < -2147483648n ? -2147483648n : minVal;
      const effMax = maxVal > 2147483647n ? 2147483647n : maxVal;
      if (nVal < effMin || nVal > effMax) throw new Error(`Out of range for ${k}`);
      fArgs[k] = Number(nVal);
    }
  }

  if (msgInfo.formatIntent === "cardinal") {
    const select = CATEGORIES[locInfo.cardinal];
    fArgs.i18nCategory = select ? select(Math.abs(fArgs[msgInfo.selectorVar])) : "other";
  }

  const bundle = getBundle(locale, useIsolating);

  const targetKey = msgInfo.formatIntent === "enum" ? (msgInfo.variantKeys[selectedVariant] || msgInfo.defaultKey) : msgInfo.resourceKey;
  const message = bundle.getMessage(targetKey);
  if (!message || !message.value) throw new Error(`Message not found: ${targetKey}`);

  const formatErrors = [];
  const output = bundle.formatPattern(message.value, fArgs, formatErrors);
  if (formatErrors.length > 0) throw new Error(`Format error: ${formatErrors.join(", ")}`);
  return output;
}
globalThis.SolstoneI18nBundles ??= Object.create(null);
globalThis.SolstoneI18nBundles["journal-window"] = Object.freeze({render, integer, cardinal, messageIds: Object.freeze(Object.keys(MESSAGES))});
})();
