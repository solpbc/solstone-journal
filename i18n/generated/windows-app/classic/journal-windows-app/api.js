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
"en-US": "m6a6f75726e616c2e77696e646f77732e6164642e646576696365 = pair a device\n\nm6a6f75726e616c2e77696e646f77732e61646d696e2e7465726d696e616c = open a terminal\n\nm6a6f75726e616c2e77696e646f77732e61646f70742e6c616e64696e672e6c696e65 = nothing moved. your journal was always here.\n\nm6a6f75726e616c2e77696e646f77732e6261636b75702e6c696e65 = backup keeps your journal safe. set it up from your journal.\n\nm6a6f75726e616c2e77696e646f77732e6261636b75702e7469746c65 = backup\n\nm6a6f75726e616c2e77696e646f77732e6272616e64 = solstone\n\nm6a6f75726e616c2e77696e646f77732e62726f777365722e6661696c6564 = your browser didn't open.\n\nm6a6f75726e616c2e77696e646f77732e62726f777365722e6c696e65 = this window moves on when you finish.\n\nm6a6f75726e616c2e77696e646f77732e62726f777365722e7469746c65 = finish setting up in your browser\n\nm6a6f75726e616c2e77696e646f77732e63616e63656c = cancel\n\nm6a6f75726e616c2e77696e646f77732e63686f6f73652e6c6f636174696f6e = choose\n\nm6a6f75726e616c2e77696e646f77732e636c6f7365 = close\n\nm6a6f75726e616c2e77696e646f77732e636f6e6669726d65642e6c696e65 = this is your journal\n\nm6a6f75726e616c2e77696e646f77732e636f6e74696e75652e627574746f6e = continue\n\nm6a6f75726e616c2e77696e646f77732e636f6e74696e75652e696e2e736f6c73746f6e65 = continue in the solstone app\n\nm6a6f75726e616c2e77696e646f77732e636f70696564 = copied \u2713\n\nm6a6f75726e616c2e77696e646f77732e636f706965642e61626f7574 = copied\n\nm6a6f75726e616c2e77696e646f77732e636f70792e61626f7574 = copy\n\nm6a6f75726e616c2e77696e646f77732e636f70792e61626f75742e6661696c6564 = couldn't copy. select the text and copy it.\n\nm6a6f75726e616c2e77696e646f77732e636f70792e6c696e6b = copy pairing link\n\nm6a6f75726e616c2e77696e646f77732e636f756e74646f776e = expires in { $time }\n\nm6a6f75726e616c2e77696e646f77732e646576696365732e656d7074792e626f6479 = pair a device and it joins your journal.\n\nm6a6f75726e616c2e77696e646f77732e646576696365732e656d7074792e7469746c65 = no devices yet\n\nm6a6f75726e616c2e77696e646f77732e646576696365732e6c6f61642e6661696c6564 = couldn't load your devices\n\nm6a6f75726e616c2e77696e646f77732e646576696365732e6c6f6164696e67 = loading devices\u2026\n\nm6a6f75726e616c2e77696e646f77732e646576696365732e6e6f742e72756e6e696e672e626f6479 = start the journal, then try again.\n\nm6a6f75726e616c2e77696e646f77732e646576696365732e6e6f742e72756e6e696e672e7469746c65 = your journal isn't running\n\nm6a6f75726e616c2e77696e646f77732e646576696365732e70656572 = another journal\n\nm6a6f75726e616c2e77696e646f77732e646576696365732e7469746c65 = devices\n\nm6a6f75726e616c2e77696e646f77732e6469736b2e75736564 = disk used\n\nm6a6f75726e616c2e77696e646f77732e646973706c61792e6e6f742e72756e6e696e67 = not running\n\nm6a6f75726e616c2e77696e646f77732e646973706c61792e6f746865722e6163636f756e74 = not running\n\nm6a6f75726e616c2e77696e646f77732e646973706c61792e72756e6e696e67 = running\n\nm6a6f75726e616c2e77696e646f77732e646973706c61792e7374617274696e67 = starting\u2026\n\nm6a6f75726e616c2e77696e646f77732e646973706c61792e73746f70706564 = stopped\n\nm6a6f75726e616c2e77696e646f77732e646973706c61792e73746f7070696e67 = stopping\u2026\n\nm6a6f75726e616c2e77696e646f77732e646f776e6c6f61642e736f6c73746f6e65 = download the solstone app for windows\n\nm6a6f75726e616c2e77696e646f77732e646f776e6c6f6164732e70726f6772657373 = { $done } of { $total }\n\nm6a6f75726e616c2e77696e646f77732e6572726f722e626f6f742e706172616d65746572 = task_installed must be a boolean\n\nm6a6f75726e616c2e77696e646f77732e6572726f722e636c692e6e6f2e726561736f6e = journal didn't say why\n\nm6a6f75726e616c2e77696e646f77732e6572726f722e636c692e72756e = couldn't run { \"solstone journal\" }: { $error }\n\nm6a6f75726e616c2e77696e646f77732e6572726f722e646f776e6c6f61642e6368616e676564 = that download isn't the one on screen\n\nm6a6f75726e616c2e77696e646f77732e6572726f722e666c6f772e696e74656e74 = unknown flow intent { $intent }\n\nm6a6f75726e616c2e77696e646f77732e6572726f722e666f6c6465722e6d697373696e67 = that folder isn't there\n\nm6a6f75726e616c2e77696e646f77732e6572726f722e666f6c6465722e7265717569726564 = choose where your journal lives\n\nm6a6f75726e616c2e77696e646f77732e6572726f722e68616e646f66662e6661696c6564 = could not continue in the solstone app. try again.\n\nm6a6f75726e616c2e77696e646f77732e6572726f722e69636f6e2e6279746573 = an icon image isn't bytes\n\nm6a6f75726e616c2e77696e646f77732e6572726f722e69636f6e2e636f756e74 = too many icon images\n\nm6a6f75726e616c2e77696e646f77732e6572726f722e69636f6e2e656d707479 = no icon images\n\nm6a6f75726e616c2e77696e646f77732e6572726f722e69636f6e2e6e6f2e696d616765 = no icon image\n\nm6a6f75726e616c2e77696e646f77732e6572726f722e69636f6e2e6e6f2e73697a65 = no icon size\n\nm6a6f75726e616c2e77696e646f77732e6572726f722e69636f6e2e706e67 = the { $side } px icon image is not a { $side } px PNG\n\nm6a6f75726e616c2e77696e646f77732e6572726f722e69636f6e2e73697a65 = unexpected icon size { $side }\n\nm6a6f75726e616c2e77696e646f77732e6572726f722e6a6f75726e616c2e61646472657373 = that isn't your journal's address\n\nm6a6f75726e616c2e77696e646f77732e6572726f722e6a6f75726e616c2e6f66666c696e65 = the journal isn't answering\n\nm6a6f75726e616c2e77696e646f77732e6572726f722e6a6f75726e616c2e726573706f6e7365 = your journal answered { $status }\n\nm6a6f75726e616c2e77696e646f77732e6572726f722e6d61726b2e6368616e676564 = that mark isn't the one on screen\n\nm6a6f75726e616c2e77696e646f77732e6572726f722e706167652e756e737570706f72746564 = that page isn't one the app opens\n\nm6a6f75726e616c2e77696e646f77732e6572726f722e70616972696e672e6c696e6b = that isn't a pairing link\n\nm6a6f75726e616c2e77696e646f77732e6572726f722e706172616d657465722e6d697373696e67 = missing on parameter\n\nm6a6f75726e616c2e77696e646f77732e6572726f722e7265646972656374 = unexpected redirect destination\n\nm6a6f75726e616c2e77696e646f77732e6572726f722e726571756573742e756e6b6e6f776e = unknown request { $command }\n\nm6a6f75726e616c2e77696e646f77732e6572726f722e726f7574652e756e6b6e6f776e = { $method } { $path } isn't a journal route the app uses\n\nm6a6f75726e616c2e77696e646f77732e6572726f722e72756e74696d652e6d697373696e67 = this PC is missing the Microsoft Edge WebView2 Runtime. install the WebView2 Runtime from Microsoft, then open journal again.\n\nm6a6f75726e616c2e77696e646f77732e6572726f722e73657475702e616374696f6e73 = native setup actions are unavailable in this build\n\nm6a6f75726e616c2e77696e646f77732e6572726f722e73657475702e636f7079 = invalid setup copy JSON: { $error }\n\nm6a6f75726e616c2e77696e646f77732e6572726f722e73657475702e73657175656e6365 = setup sequence is unavailable in this build\n\nm6a6f75726e616c2e77696e646f77732e6572726f722e73657475702e7374617274 = couldn't start setup: { $error }\n\nm6a6f75726e616c2e77696e646f77732e6572726f722e73657475702e7374617465 = the journal's setup state could not be read\n\nm6a6f75726e616c2e77696e646f77732e6572726f722e73657475702e756e66696e6973686564 = setup didn't finish\n\nm6a6f75726e616c2e77696e646f77732e6572726f722e73657475702e77616974 = setup didn't finish: { $error }\n\nm6a6f75726e616c2e77696e646f77732e6572726f722e7368656c6c2e6f70656e = windows couldn't open { $target }\n\nm6a6f75726e616c2e77696e646f77732e6572726f722e7369676e696e2e6f70656e = couldn't open the sign-in list (windows error { $opened })\n\nm6a6f75726e616c2e77696e646f77732e6572726f722e7369676e696e2e7772697465 = couldn't change the sign-in list (windows error { $result })\n\nm6a6f75726e616c2e77696e646f77732e6572726f722e7374617475732e6d697373696e67 = the journal reported no status\n\nm6a6f75726e616c2e77696e646f77732e6572726f722e7374617475732e736368656d61 = unexpected status schema { $schema }\n\nm6a6f75726e616c2e77696e646f77732e6572726f722e737465702e73617665 = could not save this step. try again.\n\nm6a6f75726e616c2e77696e646f77732e6572726f722e7465726d696e616c2e6f70656e = couldn't open a terminal: { $error }\n\nm6a6f75726e616c2e77696e646f77732e6572726f722e7468696e6b696e672e63686f696365 = unknown thinking choice\n\nm6a6f75726e616c2e77696e646f77732e6572726f722e7468696e6b696e672e756e617661696c61626c65 = local thinking is unavailable on this device\n\nm6a6f75726e616c2e77696e646f77732e6572726f722e77696e646f772e6f70656e = the journal app couldn't open its window.{ \"\\u000a\" }{ $reason }\n\nm6a6f75726e616c2e77696e646f77732e6865616c7468 = health\n\nm6a6f75726e616c2e77696e646f77732e6865616c746879 = healthy\n\nm6a6f75726e616c2e77696e646f77732e686f6d652e6c696e65 = this app keeps your journal running on this PC. your journal itself opens in your browser.\n\nm6a6f75726e616c2e77696e646f77732e686f6d652e7469746c65 = your journal, at a glance\n\nm6a6f75726e616c2e77696e646f77732e6a6f75726e616c2e666f756e64 = journal found\n\nm6a6f75726e616c2e77696e646f77732e6c6173742e7365656e = last seen { $relative }\n\nm6a6f75726e616c2e77696e646f77732e6c6173742e7365656e2e6a7573742e6e6f77 = last seen just now\n\nm6a6f75726e616c2e77696e646f77732e6c6173742e7365656e2e6a7573742e6e6f772e706169726564 = last seen just now \u00b7 paired { $date }\n\nm6a6f75726e616c2e77696e646f77732e6c6173742e7365656e2e706169726564 = last seen { $relative } \u00b7 paired { $date }\n\nm6a6f75726e616c2e77696e646f77732e6c6173742e7365656e2e756e6b6e6f776e = last seen unknown\n\nm6a6f75726e616c2e77696e646f77732e6c6173742e7365656e2e756e6b6e6f776e2e706169726564 = last seen unknown \u00b7 paired { $date }\n\nm6a6f75726e616c2e77696e646f77732e6c696e6b2e657870697265642e626f6479 = open a fresh pairing link to pair a device.\n\nm6a6f75726e616c2e77696e646f77732e6c696e6b2e657870697265642e7469746c65 = pairing link expired\n\nm6a6f75726e616c2e77696e646f77732e6c6f636174696f6e = location\n\nm6a6f75726e616c2e77696e646f77732e6c6f636174696f6e2e7469746c65 = let's create your journal on this PC\n\nm6a6f75726e616c2e77696e646f77732e6d61726b2e656d7074792e6e616d65 = your journal, not set up yet\n\nm6a6f75726e616c2e77696e646f77732e6d61726b2e6e616d65 = { $first }, { $second } \u00b7 { $words }\n\nm6a6f75726e616c2e77696e646f77732e6d61726b2e706c616365686f6c6465722e776f726473 = your\u00b7journal\n\nm6a6f75726e616c2e77696e646f77732e6d61726b2e756e617661696c61626c652e6e616d65 = your journal's mark, unavailable right now\n\nm6a6f75726e616c2e77696e646f77732e6d61726b2e756e617661696c61626c652e776f726473 = mark\u00b7unavailable\n\nm6a6f75726e616c2e77696e646f77732e6d6f64656c732e646f776e6c6f6164696e67 = downloading\u2026\n\nm6a6f75726e616c2e77696e646f77732e6d6f64656c732e6d697373696e67 = repair or reinstall the journal app, then restart your journal.\n\nm6a6f75726e616c2e77696e646f77732e6e6574776f726b2e636c6f73652e636f6e6669726d = close your journal to devices on your network? any device connected over your network now will disconnect.\n\nm6a6f75726e616c2e77696e646f77732e6e6574776f726b2e636c6f73652e637461 = close to devices on your network\n\nm6a6f75726e616c2e77696e646f77732e6e6574776f726b2e636c6f736564 = closed to devices on your network\n\nm6a6f75726e616c2e77696e646f77732e6e6574776f726b2e636c6f7365642e68656c70 = the solstone app on this PC can still pair from here, and so can another device once your private network is on. to pair a phone or another computer over your own network, open your journal to it.\n\nm6a6f75726e616c2e77696e646f77732e6e6574776f726b2e636c6f7365642e68656c702e77696e646f7773 = the solstone app on this PC can still pair from here, and so can another device once your private network is on. to pair a phone or another computer over your own network, open your journal to it. windows may then ask whether journal can use the network, and on a standard account an administrator has to allow it.\n\nm6a6f75726e616c2e77696e646f77732e6e6574776f726b2e6661696c6564 = couldn't change whether your journal is open to devices on your network. try again.\n\nm6a6f75726e616c2e77696e646f77732e6e6574776f726b2e6e6f742e726561636861626c65 = devices can't reach your journal right now.\n\nm6a6f75726e616c2e77696e646f77732e6e6574776f726b2e6f70656e = open to devices on your network\n\nm6a6f75726e616c2e77696e646f77732e6e6574776f726b2e6f70656e2e6167656e7473 = your journal stays open to devices on your network while \"agents on your network\" is on.\n\nm6a6f75726e616c2e77696e646f77732e6e6574776f726b2e6f70656e2e6167656e74732e77696e646f7773 = your journal stays open to devices on your network while \"agents on your network\" is on. if windows didn't ask and a device still can't reach your journal, an earlier choice in windows may be blocking journal. an administrator can allow it in Windows Security, under Firewall & network protection \u203a Allow an app through firewall.\n\nm6a6f75726e616c2e77696e646f77732e6e6574776f726b2e6f70656e2e637461 = open to devices on your network\n\nm6a6f75726e616c2e77696e646f77732e6e6574776f726b2e6f70656e2e646972656374 = devices on the same network can pair with your journal directly.\n\nm6a6f75726e616c2e77696e646f77732e6e6574776f726b2e6f70656e2e6469726563742e77696e646f7773 = devices on the same network can pair with your journal directly. if windows didn't ask and a device still can't reach your journal, an earlier choice in windows may be blocking journal. an administrator can allow it in Windows Security, under Firewall & network protection \u203a Allow an app through firewall.\n\nm6a6f75726e616c2e77696e646f77732e6e6574776f726b2e6f70656e2e756e74696c = your journal stays open to devices on your network while \"agents on your network\" is on. it closes to them when you turn that off in agents, in your journal.\n\nm6a6f75726e616c2e77696e646f77732e6e6574776f726b2e6f70656e2e756e74696c2e77696e646f7773 = your journal stays open to devices on your network while \"agents on your network\" is on. it closes to them when you turn that off in agents, in your journal. if windows didn't ask and a device still can't reach your journal, an earlier choice in windows may be blocking journal. an administrator can allow it in Windows Security, under Firewall & network protection \u203a Allow an app through firewall.\n\nm6a6f75726e616c2e77696e646f77732e6e657665722e636f6e6e6563746564 = never connected\n\nm6a6f75726e616c2e77696e646f77732e6e657665722e636f6e6e65637465642e706169726564 = never connected \u00b7 paired { $date }\n\nm6a6f75726e616c2e77696e646f77732e6f70656e2e6261636b7570 = open backup\n\nm6a6f75726e616c2e77696e646f77732e6f70656e2e62726f77736572 = open the setup page\n\nm6a6f75726e616c2e77696e646f77732e6f70656e2e66726573682e6c696e6b = open a fresh pairing link\n\nm6a6f75726e616c2e77696e646f77732e6f70656e2e6a6f75726e616c = open your journal\n\nm6a6f75726e616c2e77696e646f77732e6f746865722e6163636f756e742e626f6479 = one journal runs on a computer at a time, and another account here is running its own. yours can start once that account stops its journal or logs off.\n\nm6a6f75726e616c2e77696e646f77732e6f746865722e6163636f756e742e7469746c65 = another account's journal is running on this computer\n\nm6a6f75726e616c2e77696e646f77732e706169722e746869732e7063 = pair the solstone app on this PC\n\nm6a6f75726e616c2e77696e646f77732e7061697265642e646f6e65 = done\n\nm6a6f75726e616c2e77696e646f77732e7061697265642e68656164696e67 = \"{ $label }\" can now reach your journal\n\nm6a6f75726e616c2e77696e646f77732e7061697265642e68656164696e672e756e6e616d6564 = a new device can now reach your journal\n\nm6a6f75726e616c2e77696e646f77732e7061697265642e6e6f7465 = on that device, check it shows this same mark, then answer there. didn't pair it?\n\nm6a6f75726e616c2e77696e646f77732e7061697265642e6e6f74652e616e797768657265 = on that device, check it shows this same mark, then answer there. it can reach your journal from anywhere. didn't pair it?\n\nm6a6f75726e616c2e77696e646f77732e7061697265642e6e6f74652e746869732e7063 = if the solstone app asks, check it shows this same mark, then answer there. didn't pair it?\n\nm6a6f75726e616c2e77696e646f77732e7061697265642e72656d6f76652e6661696c6564 = couldn't unpair this device. try again, or remove it under devices.\n\nm6a6f75726e616c2e77696e646f77732e7061697265642e72656d6f766564 = unpaired. the device can't reach your journal until it's paired again.\n\nm6a6f75726e616c2e77696e646f77732e7061697265642e756e70616972 = unpair this device\n\nm6a6f75726e616c2e77696e646f77732e70616972696e672e636c6f736564 = your journal is closed to devices on your network, so another device can't reach it directly yet. open your journal to devices on your network, or turn on your private network to pair from anywhere. the solstone app on this PC can still pair.\n\nm6a6f75726e616c2e77696e646f77732e70616972696e672e636f6465 = pairing code\n\nm6a6f75726e616c2e77696e646f77732e70616972696e672e6661696c65642e7469746c65 = couldn't open pairing\n\nm6a6f75726e616c2e77696e646f77732e70616972696e672e696e737472756374696f6e73 = scan this code or open the pairing link on the device you want to add.\n\nm6a6f75726e616c2e77696e646f77732e70616972696e672e6f70656e696e67 = opening pairing\u2026\n\nm6a6f75726e616c2e77696e646f77732e70616972696e672e746869732e7063 = this pairing link works only for the solstone app on this PC. it's copied, so paste it into the solstone app to pair.\n\nm6a6f75726e616c2e77696e646f77732e70616972696e672e746869732e70632e636f7079 = this pairing link works only for the solstone app on this PC. copy it, then paste it into the solstone app to pair.\n\nm6a6f75726e616c2e77696e646f77732e70616e65732e6261636b7570 = backup\n\nm6a6f75726e616c2e77696e646f77732e70616e65732e64657669636573 = devices\n\nm6a6f75726e616c2e77696e646f77732e70616e65732e686f6d65 = home\n\nm6a6f75726e616c2e77696e646f77732e70616e65732e6a6f75726e616c = location\n\nm6a6f75726e616c2e77696e646f77732e70616e65732e72756e = run state\n\nm6a6f75726e616c2e77696e646f77732e70616e65732e73746172747570 = startup\n\nm6a6f75726e616c2e77696e646f77732e70616e65732e75706461746573 = updates\n\nm6a6f75726e616c2e77696e646f77732e706565722e6a6f75726e616c73 = peer journals\n\nm6a6f75726e616c2e77696e646f77732e71756974 = quit journal\n\nm6a6f75726e616c2e77696e646f77732e7175697474696e67 = stopping your journal\u2026\n\nm6a6f75726e616c2e77696e646f77732e72656c61792e7365747570 = turn on your private network in your journal \u2192\n\nm6a6f75726e616c2e77696e646f77732e72656d6f7665 = remove\n\nm6a6f75726e616c2e77696e646f77732e72656d6f76652e626f6479 = { $name } loses access to your journal. you can pair it again later.\n\nm6a6f75726e616c2e77696e646f77732e72656d6f76652e6661696c6564 = couldn't remove this device\n\nm6a6f75726e616c2e77696e646f77732e72656d6f76652e7469746c65 = remove { $name }?\n\nm6a6f75726e616c2e77696e646f77732e72657374617274 = restart\n\nm6a6f75726e616c2e77696e646f77732e72756e2e73746174652e6c696e6b = run state \u2192\n\nm6a6f75726e616c2e77696e646f77732e73657475702e6170702e7465726d = the solstone app\n\nm6a6f75726e616c2e77696e646f77732e73657475702e6170702e74657874 = runs on this PC. your microphone, screen and computer sound each stay off until you turn them on.\n\nm6a6f75726e616c2e77696e646f77732e73657475702e62616e64 = your journal\n\nm6a6f75726e616c2e77696e646f77732e73657475702e666974732e7469746c65 = what this sets up\n\nm6a6f75726e616c2e77696e646f77732e73657475702e666f6c6465722e6c6162656c = journal folder\n\nm6a6f75726e616c2e77696e646f77732e73657475702e666f6c6465722e74657874 = everything your journal keeps stays in this folder, on this PC.\n\nm6a6f75726e616c2e77696e646f77732e73657475702e666f6c6465722e7469746c65 = where your journal lives\n\nm6a6f75726e616c2e77696e646f77732e73657475702e6a6f75726e616c2e7465726d = your journal\n\nm6a6f75726e616c2e77696e646f77732e73657475702e6a6f75726e616c2e74657874 = lives on this PC and keeps everything that goes into it.\n\nm6a6f75726e616c2e77696e646f77732e73657475702e6c6f63616c2e64657363 = model requests stay on this PC. continue starts the download.\n\nm6a6f75726e616c2e77696e646f77732e73657475702e6f6e2e746869732e7063 = on this PC\n\nm6a6f75726e616c2e77696e646f77732e73657475702e7369676e2e696e = start automatically after you sign in to this PC.\n\nm6a6f75726e616c2e77696e646f77732e73657475702e7374617274 = start setup\n\nm6a6f75726e616c2e77696e646f77732e73657475702e73746570 = step { $n } of 8\n\nm6a6f75726e616c2e77696e646f77732e73657475702e73746f70706564 = setup stopped before it finished.\n\nm6a6f75726e616c2e77696e646f77732e73657475702e7375627469746c65 = this can take a minute.\n\nm6a6f75726e616c2e77696e646f77732e73657475702e7468696e6b696e672e6c656164 = your journal stays on this PC whatever you choose. this only decides where thinking happens.\n\nm6a6f75726e616c2e77696e646f77732e73657475702e7468696e6b696e672e7465726d = thinking\n\nm6a6f75726e616c2e77696e646f77732e73657475702e7468696e6b696e672e74657874 = helps your journal make sense of what it keeps. you choose where during setup, or later.\n\nm6a6f75726e616c2e77696e646f77732e73657475702e7468696e6b696e672e7469746c65 = where does the thinking happen?\n\nm6a6f75726e616c2e77696e646f77732e73657475702e7469746c65 = setting up your journal\n\nm6a6f75726e616c2e77696e646f77732e73686f772e696e2e6578706c6f726572 = show in File Explorer\n\nm6a6f75726e616c2e77696e646f77732e7369676e696e2e6661696c6564 = sign-in launch failed\n\nm6a6f75726e616c2e77696e646f77732e73697a652e6279746573 = { $i18nCategory ->\n    [one] { NUMBER($quantity, useGrouping: 0) } bytes\n   *[other] { NUMBER($quantity, useGrouping: 0) } bytes\n}\n\nm6a6f75726e616c2e77696e646f77732e73697a652e6762 = { $value } GB\n\nm6a6f75726e616c2e77696e646f77732e73697a652e6b62 = { $value } KB\n\nm6a6f75726e616c2e77696e646f77732e73697a652e6d62 = { $value } MB\n\nm6a6f75726e616c2e77696e646f77732e73697a652e7462 = { $value } TB\n\nm6a6f75726e616c2e77696e646f77732e7374617274 = start\n\nm6a6f75726e616c2e77696e646f77732e73746172742e61742e7369676e2e696e = start the journal when you sign in\n\nm6a6f75726e616c2e77696e646f77732e737461727475702e7469746c65 = startup\n\nm6a6f75726e616c2e77696e646f77732e7374617475732e6661696c6564 = couldn't read journal status\n\nm6a6f75726e616c2e77696e646f77732e73746570732e627261696e = setting up the model\n\nm6a6f75726e616c2e77696e646f77732e73746570732e646f63746f72 = checking this PC\n\nm6a6f75726e616c2e77696e646f77732e73746570732e696e7374616c6c2e6d6f64656c73 = downloading models\n\nm6a6f75726e616c2e77696e646f77732e73746570732e6a6f75726e616c = preparing your journal\n\nm6a6f75726e616c2e77696e646f77732e73746570732e73657276696365 = starting your journal\n\nm6a6f75726e616c2e77696e646f77732e73746f70 = stop\n\nm6a6f75726e616c2e77696e646f77732e7469746c65 = journal\n\nm6a6f75726e616c2e77696e646f77732e7472792e616761696e = try again\n\nm6a6f75726e616c2e77696e646f77732e756e6b6e6f776e = unknown\n\nm6a6f75726e616c2e77696e646f77732e756e6e616d65642e646576696365 = unnamed device\n\nm6a6f75726e616c2e77696e646f77732e757064617465732e6175746f2e636865636b = check for updates automatically\n\nm6a6f75726e616c2e77696e646f77732e757064617465732e6175746f2e646f776e6c6f6164 = download updates in the background\n\nm6a6f75726e616c2e77696e646f77732e757064617465732e6175746f6d61746963 = automatic updates\n\nm6a6f75726e616c2e77696e646f77732e757064617465732e617661696c61626c652e7375627469746c65 = the journal app will download it in the background when this connection allows it.\n\nm6a6f75726e616c2e77696e646f77732e757064617465732e617661696c61626c652e7469746c65 = version { $target } is available\n\nm6a6f75726e616c2e77696e646f77732e757064617465732e636865636b2e616761696e = check again\n\nm6a6f75726e616c2e77696e646f77732e757064617465732e636865636b2e6e6f77 = check now\n\nm6a6f75726e616c2e77696e646f77732e757064617465732e636865636b696e67 = checking\u2026\n\nm6a6f75726e616c2e77696e646f77732e757064617465732e636865636b696e672e776974682e646f776e6c6f61646564 = version { $downloaded } is downloaded. checking for a newer one\u2026\n\nm6a6f75726e616c2e77696e646f77732e757064617465732e646179 = every day\n\nm6a6f75726e616c2e77696e646f77732e757064617465732e646566657272616c2e6d657465726564 = this connection is metered, so the download is waiting.\n\nm6a6f75726e616c2e77696e646f77732e757064617465732e646566657272616c2e72657374726963746564 = this connection does not allow a background download, so it is waiting.\n\nm6a6f75726e616c2e77696e646f77732e757064617465732e646566657272616c2e726f616d696e67 = this connection is roaming, so the download is waiting.\n\nm6a6f75726e616c2e77696e646f77732e757064617465732e646566657272616c2e756e6b6e6f776e = the connection is not known to allow a background download, so it is waiting.\n\nm6a6f75726e616c2e77696e646f77732e757064617465732e646f776e6c6f6164 = download\n\nm6a6f75726e616c2e77696e646f77732e757064617465732e646f776e6c6f6164696e672e72657461696e732e646f776e6c6f61646564 = version { $downloaded } stays downloaded until the new one finishes.\n\nm6a6f75726e616c2e77696e646f77732e757064617465732e646f776e6c6f6164696e672e7469746c65 = downloading { $target }\n\nm6a6f75726e616c2e77696e646f77732e757064617465732e6661696c7572652e636865636b2e626f6479 = the journal could not finish the check.\n\nm6a6f75726e616c2e77696e646f77732e757064617465732e6661696c7572652e636865636b2e72657461696e65642e626f6479 = the journal could not finish the check. the downloaded update is still here.\n\nm6a6f75726e616c2e77696e646f77732e757064617465732e6661696c7572652e636865636b2e7469746c65 = the check failed\n\nm6a6f75726e616c2e77696e646f77732e757064617465732e6661696c7572652e646f776e6c6f61642e626f6479 = the download did not finish.\n\nm6a6f75726e616c2e77696e646f77732e757064617465732e6661696c7572652e646f776e6c6f61642e72657461696e65642e626f6479 = the download did not finish. version { $downloaded } is still downloaded.\n\nm6a6f75726e616c2e77696e646f77732e757064617465732e6661696c7572652e646f776e6c6f61642e7469746c65 = the download failed\n\nm6a6f75726e616c2e77696e646f77732e757064617465732e6661696c7572652e696e7374616c6c2e626f6479 = the downloaded update is still here. the journal installs it the next time the journal app starts.\n\nm6a6f75726e616c2e77696e646f77732e757064617465732e6661696c7572652e696e7374616c6c2e6d697373696e67 = the downloaded update is no longer available. check for an update again.\n\nm6a6f75726e616c2e77696e646f77732e757064617465732e6661696c7572652e696e7374616c6c2e7469746c65 = the update did not finish\n\nm6a6f75726e616c2e77696e646f77732e757064617465732e6661696c7572652e73746f702e626f6479 = the update could not start. the downloaded update is still here.\n\nm6a6f75726e616c2e77696e646f77732e757064617465732e6661696c7572652e73746f702e7469746c65 = the update did not start\n\nm6a6f75726e616c2e77696e646f77732e757064617465732e686561646572 = journal { $version }\n\nm6a6f75726e616c2e77696e646f77732e757064617465732e686f772e6f6674656e = how often\n\nm6a6f75726e616c2e77696e646f77732e757064617465732e696e7374616c6c = install now\n\nm6a6f75726e616c2e77696e646f77732e757064617465732e696e7374616c6c696e672e7375627469746c65 = the journal app is applying the update.\n\nm6a6f75726e616c2e77696e646f77732e757064617465732e696e7374616c6c696e672e7469746c65 = installing { $downloaded }\n\nm6a6f75726e616c2e77696e646f77732e757064617465732e6a7573742e6e6f77 = just now\n\nm6a6f75726e616c2e77696e646f77732e757064617465732e6c6173742e636865636b6564 = last checked { $relative }\n\nm6a6f75726e616c2e77696e646f77732e757064617465732e6d6f6e7468 = every month\n\nm6a6f75726e616c2e77696e646f77732e757064617465732e6e657665722e636865636b6564 = never checked for updates\n\nm6a6f75726e616c2e77696e646f77732e757064617465732e70726976616379 = no usage data is sent. a check fetches the version list. a background download runs only on an unrestricted connection.\n\nm6a6f75726e616c2e77696e646f77732e757064617465732e72656164792e7469746c65 = version { $downloaded } is downloaded. the journal installs it the next time the journal app starts.\n\nm6a6f75726e616c2e77696e646f77732e757064617465732e72657472792e636865636b = retry check\n\nm6a6f75726e616c2e77696e646f77732e757064617465732e72657472792e646f776e6c6f6164 = retry download\n\nm6a6f75726e616c2e77696e646f77732e757064617465732e756e617661696c61626c652e7375627469746c65 = this build cannot check for updates on its own.\n\nm6a6f75726e616c2e77696e646f77732e757064617465732e756e617661696c61626c652e7469746c65 = updates are unavailable\n\nm6a6f75726e616c2e77696e646f77732e757064617465732e75702e746f2e64617465 = the journal is up to date\n\nm6a6f75726e616c2e77696e646f77732e757064617465732e7765656b = every week\n\nm6a6f75726e616c2e77696e646f77732e776861742e68617070656e6564 = what happened\n\nm6a6f75726e616c2e77696e646f77732e796f75722e64657669636573 = your devices\n\n",
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
  "journal.windows.add.device": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e6164642e646576696365",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.admin.terminal": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e61646d696e2e7465726d696e616c",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.adopt.landing.line": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e61646f70742e6c616e64696e672e6c696e65",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.backup.line": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e6261636b75702e6c696e65",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.backup.title": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e6261636b75702e7469746c65",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.brand": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e6272616e64",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.browser.failed": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e62726f777365722e6661696c6564",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.browser.line": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e62726f777365722e6c696e65",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.browser.title": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e62726f777365722e7469746c65",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.cancel": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e63616e63656c",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.choose.location": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e63686f6f73652e6c6f636174696f6e",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.close": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e636c6f7365",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.confirmed.line": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e636f6e6669726d65642e6c696e65",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.continue.button": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e636f6e74696e75652e627574746f6e",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.continue.in.solstone": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e636f6e74696e75652e696e2e736f6c73746f6e65",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.copied": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e636f70696564",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.copied.about": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e636f706965642e61626f7574",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.copy.about": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e636f70792e61626f7574",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.copy.about.failed": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e636f70792e61626f75742e6661696c6564",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.copy.link": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e636f70792e6c696e6b",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.countdown": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e636f756e74646f776e",
    formatIntent: "text",
    arguments: [
      { name: "time", type: "string", min: null, max: null, numericStyle: "ungrouped" },
    ],
  },
  "journal.windows.devices.empty.body": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e646576696365732e656d7074792e626f6479",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.devices.empty.title": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e646576696365732e656d7074792e7469746c65",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.devices.load.failed": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e646576696365732e6c6f61642e6661696c6564",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.devices.loading": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e646576696365732e6c6f6164696e67",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.devices.not.running.body": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e646576696365732e6e6f742e72756e6e696e672e626f6479",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.devices.not.running.title": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e646576696365732e6e6f742e72756e6e696e672e7469746c65",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.devices.peer": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e646576696365732e70656572",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.devices.title": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e646576696365732e7469746c65",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.disk.used": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e6469736b2e75736564",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.display.not.running": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e646973706c61792e6e6f742e72756e6e696e67",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.display.other.account": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e646973706c61792e6f746865722e6163636f756e74",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.display.running": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e646973706c61792e72756e6e696e67",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.display.starting": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e646973706c61792e7374617274696e67",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.display.stopped": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e646973706c61792e73746f70706564",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.display.stopping": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e646973706c61792e73746f7070696e67",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.download.solstone": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e646f776e6c6f61642e736f6c73746f6e65",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.downloads.progress": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e646f776e6c6f6164732e70726f6772657373",
    formatIntent: "text",
    arguments: [
      { name: "done", type: "string", min: null, max: null, numericStyle: "ungrouped" },
      { name: "total", type: "string", min: null, max: null, numericStyle: "ungrouped" },
    ],
  },
  "journal.windows.error.boot.parameter": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e6572726f722e626f6f742e706172616d65746572",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.error.cli.no.reason": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e6572726f722e636c692e6e6f2e726561736f6e",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.error.cli.run": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e6572726f722e636c692e72756e",
    formatIntent: "text",
    arguments: [
      { name: "error", type: "string", min: null, max: null, numericStyle: "ungrouped" },
    ],
  },
  "journal.windows.error.download.changed": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e6572726f722e646f776e6c6f61642e6368616e676564",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.error.flow.intent": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e6572726f722e666c6f772e696e74656e74",
    formatIntent: "text",
    arguments: [
      { name: "intent", type: "string", min: null, max: null, numericStyle: "ungrouped" },
    ],
  },
  "journal.windows.error.folder.missing": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e6572726f722e666f6c6465722e6d697373696e67",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.error.folder.required": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e6572726f722e666f6c6465722e7265717569726564",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.error.handoff.failed": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e6572726f722e68616e646f66662e6661696c6564",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.error.icon.bytes": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e6572726f722e69636f6e2e6279746573",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.error.icon.count": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e6572726f722e69636f6e2e636f756e74",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.error.icon.empty": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e6572726f722e69636f6e2e656d707479",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.error.icon.no.image": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e6572726f722e69636f6e2e6e6f2e696d616765",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.error.icon.no.size": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e6572726f722e69636f6e2e6e6f2e73697a65",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.error.icon.png": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e6572726f722e69636f6e2e706e67",
    formatIntent: "text",
    arguments: [
      { name: "side", type: "string", min: null, max: null, numericStyle: "ungrouped" },
    ],
  },
  "journal.windows.error.icon.size": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e6572726f722e69636f6e2e73697a65",
    formatIntent: "text",
    arguments: [
      { name: "side", type: "string", min: null, max: null, numericStyle: "ungrouped" },
    ],
  },
  "journal.windows.error.journal.address": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e6572726f722e6a6f75726e616c2e61646472657373",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.error.journal.offline": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e6572726f722e6a6f75726e616c2e6f66666c696e65",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.error.journal.response": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e6572726f722e6a6f75726e616c2e726573706f6e7365",
    formatIntent: "text",
    arguments: [
      { name: "status", type: "string", min: null, max: null, numericStyle: "ungrouped" },
    ],
  },
  "journal.windows.error.mark.changed": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e6572726f722e6d61726b2e6368616e676564",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.error.page.unsupported": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e6572726f722e706167652e756e737570706f72746564",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.error.pairing.link": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e6572726f722e70616972696e672e6c696e6b",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.error.parameter.missing": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e6572726f722e706172616d657465722e6d697373696e67",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.error.redirect": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e6572726f722e7265646972656374",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.error.request.unknown": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e6572726f722e726571756573742e756e6b6e6f776e",
    formatIntent: "text",
    arguments: [
      { name: "command", type: "string", min: null, max: null, numericStyle: "ungrouped" },
    ],
  },
  "journal.windows.error.route.unknown": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e6572726f722e726f7574652e756e6b6e6f776e",
    formatIntent: "text",
    arguments: [
      { name: "method", type: "string", min: null, max: null, numericStyle: "ungrouped" },
      { name: "path", type: "string", min: null, max: null, numericStyle: "ungrouped" },
    ],
  },
  "journal.windows.error.runtime.missing": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e6572726f722e72756e74696d652e6d697373696e67",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.error.setup.actions": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e6572726f722e73657475702e616374696f6e73",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.error.setup.copy": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e6572726f722e73657475702e636f7079",
    formatIntent: "text",
    arguments: [
      { name: "error", type: "string", min: null, max: null, numericStyle: "ungrouped" },
    ],
  },
  "journal.windows.error.setup.sequence": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e6572726f722e73657475702e73657175656e6365",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.error.setup.start": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e6572726f722e73657475702e7374617274",
    formatIntent: "text",
    arguments: [
      { name: "error", type: "string", min: null, max: null, numericStyle: "ungrouped" },
    ],
  },
  "journal.windows.error.setup.state": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e6572726f722e73657475702e7374617465",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.error.setup.unfinished": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e6572726f722e73657475702e756e66696e6973686564",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.error.setup.wait": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e6572726f722e73657475702e77616974",
    formatIntent: "text",
    arguments: [
      { name: "error", type: "string", min: null, max: null, numericStyle: "ungrouped" },
    ],
  },
  "journal.windows.error.shell.open": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e6572726f722e7368656c6c2e6f70656e",
    formatIntent: "text",
    arguments: [
      { name: "target", type: "string", min: null, max: null, numericStyle: "ungrouped" },
    ],
  },
  "journal.windows.error.signin.open": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e6572726f722e7369676e696e2e6f70656e",
    formatIntent: "text",
    arguments: [
      { name: "opened", type: "string", min: null, max: null, numericStyle: "ungrouped" },
    ],
  },
  "journal.windows.error.signin.write": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e6572726f722e7369676e696e2e7772697465",
    formatIntent: "text",
    arguments: [
      { name: "result", type: "string", min: null, max: null, numericStyle: "ungrouped" },
    ],
  },
  "journal.windows.error.status.missing": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e6572726f722e7374617475732e6d697373696e67",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.error.status.schema": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e6572726f722e7374617475732e736368656d61",
    formatIntent: "text",
    arguments: [
      { name: "schema", type: "string", min: null, max: null, numericStyle: "ungrouped" },
    ],
  },
  "journal.windows.error.step.save": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e6572726f722e737465702e73617665",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.error.terminal.open": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e6572726f722e7465726d696e616c2e6f70656e",
    formatIntent: "text",
    arguments: [
      { name: "error", type: "string", min: null, max: null, numericStyle: "ungrouped" },
    ],
  },
  "journal.windows.error.thinking.choice": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e6572726f722e7468696e6b696e672e63686f696365",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.error.thinking.unavailable": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e6572726f722e7468696e6b696e672e756e617661696c61626c65",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.error.window.open": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e6572726f722e77696e646f772e6f70656e",
    formatIntent: "text",
    arguments: [
      { name: "reason", type: "string", min: null, max: null, numericStyle: "ungrouped" },
    ],
  },
  "journal.windows.health": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e6865616c7468",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.healthy": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e6865616c746879",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.home.line": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e686f6d652e6c696e65",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.home.title": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e686f6d652e7469746c65",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.journal.found": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e6a6f75726e616c2e666f756e64",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.last.seen": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e6c6173742e7365656e",
    formatIntent: "text",
    arguments: [
      { name: "relative", type: "string", min: null, max: null, numericStyle: "ungrouped" },
    ],
  },
  "journal.windows.last.seen.just.now": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e6c6173742e7365656e2e6a7573742e6e6f77",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.last.seen.just.now.paired": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e6c6173742e7365656e2e6a7573742e6e6f772e706169726564",
    formatIntent: "text",
    arguments: [
      { name: "date", type: "string", min: null, max: null, numericStyle: "ungrouped" },
    ],
  },
  "journal.windows.last.seen.paired": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e6c6173742e7365656e2e706169726564",
    formatIntent: "text",
    arguments: [
      { name: "relative", type: "string", min: null, max: null, numericStyle: "ungrouped" },
      { name: "date", type: "string", min: null, max: null, numericStyle: "ungrouped" },
    ],
  },
  "journal.windows.last.seen.unknown": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e6c6173742e7365656e2e756e6b6e6f776e",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.last.seen.unknown.paired": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e6c6173742e7365656e2e756e6b6e6f776e2e706169726564",
    formatIntent: "text",
    arguments: [
      { name: "date", type: "string", min: null, max: null, numericStyle: "ungrouped" },
    ],
  },
  "journal.windows.link.expired.body": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e6c696e6b2e657870697265642e626f6479",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.link.expired.title": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e6c696e6b2e657870697265642e7469746c65",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.location": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e6c6f636174696f6e",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.location.title": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e6c6f636174696f6e2e7469746c65",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.mark.empty.name": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e6d61726b2e656d7074792e6e616d65",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.mark.name": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e6d61726b2e6e616d65",
    formatIntent: "text",
    arguments: [
      { name: "first", type: "string", min: null, max: null, numericStyle: "ungrouped" },
      { name: "second", type: "string", min: null, max: null, numericStyle: "ungrouped" },
      { name: "words", type: "string", min: null, max: null, numericStyle: "ungrouped" },
    ],
  },
  "journal.windows.mark.placeholder.words": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e6d61726b2e706c616365686f6c6465722e776f726473",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.mark.unavailable.name": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e6d61726b2e756e617661696c61626c652e6e616d65",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.mark.unavailable.words": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e6d61726b2e756e617661696c61626c652e776f726473",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.models.downloading": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e6d6f64656c732e646f776e6c6f6164696e67",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.models.missing": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e6d6f64656c732e6d697373696e67",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.network.close.confirm": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e6e6574776f726b2e636c6f73652e636f6e6669726d",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.network.close.cta": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e6e6574776f726b2e636c6f73652e637461",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.network.closed": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e6e6574776f726b2e636c6f736564",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.network.closed.help": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e6e6574776f726b2e636c6f7365642e68656c70",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.network.closed.help.windows": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e6e6574776f726b2e636c6f7365642e68656c702e77696e646f7773",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.network.failed": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e6e6574776f726b2e6661696c6564",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.network.not.reachable": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e6e6574776f726b2e6e6f742e726561636861626c65",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.network.open": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e6e6574776f726b2e6f70656e",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.network.open.agents": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e6e6574776f726b2e6f70656e2e6167656e7473",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.network.open.agents.windows": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e6e6574776f726b2e6f70656e2e6167656e74732e77696e646f7773",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.network.open.cta": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e6e6574776f726b2e6f70656e2e637461",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.network.open.direct": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e6e6574776f726b2e6f70656e2e646972656374",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.network.open.direct.windows": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e6e6574776f726b2e6f70656e2e6469726563742e77696e646f7773",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.network.open.until": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e6e6574776f726b2e6f70656e2e756e74696c",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.network.open.until.windows": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e6e6574776f726b2e6f70656e2e756e74696c2e77696e646f7773",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.never.connected": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e6e657665722e636f6e6e6563746564",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.never.connected.paired": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e6e657665722e636f6e6e65637465642e706169726564",
    formatIntent: "text",
    arguments: [
      { name: "date", type: "string", min: null, max: null, numericStyle: "ungrouped" },
    ],
  },
  "journal.windows.open.backup": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e6f70656e2e6261636b7570",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.open.browser": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e6f70656e2e62726f77736572",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.open.fresh.link": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e6f70656e2e66726573682e6c696e6b",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.open.journal": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e6f70656e2e6a6f75726e616c",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.other.account.body": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e6f746865722e6163636f756e742e626f6479",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.other.account.title": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e6f746865722e6163636f756e742e7469746c65",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.pair.this.pc": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e706169722e746869732e7063",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.paired.done": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e7061697265642e646f6e65",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.paired.heading": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e7061697265642e68656164696e67",
    formatIntent: "text",
    arguments: [
      { name: "label", type: "string", min: null, max: null, numericStyle: "ungrouped" },
    ],
  },
  "journal.windows.paired.heading.unnamed": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e7061697265642e68656164696e672e756e6e616d6564",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.paired.note": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e7061697265642e6e6f7465",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.paired.note.anywhere": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e7061697265642e6e6f74652e616e797768657265",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.paired.note.this.pc": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e7061697265642e6e6f74652e746869732e7063",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.paired.remove.failed": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e7061697265642e72656d6f76652e6661696c6564",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.paired.removed": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e7061697265642e72656d6f766564",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.paired.unpair": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e7061697265642e756e70616972",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.pairing.closed": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e70616972696e672e636c6f736564",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.pairing.code": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e70616972696e672e636f6465",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.pairing.failed.title": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e70616972696e672e6661696c65642e7469746c65",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.pairing.instructions": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e70616972696e672e696e737472756374696f6e73",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.pairing.opening": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e70616972696e672e6f70656e696e67",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.pairing.this.pc": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e70616972696e672e746869732e7063",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.pairing.this.pc.copy": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e70616972696e672e746869732e70632e636f7079",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.panes.backup": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e70616e65732e6261636b7570",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.panes.devices": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e70616e65732e64657669636573",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.panes.home": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e70616e65732e686f6d65",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.panes.journal": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e70616e65732e6a6f75726e616c",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.panes.run": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e70616e65732e72756e",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.panes.startup": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e70616e65732e73746172747570",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.panes.updates": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e70616e65732e75706461746573",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.peer.journals": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e706565722e6a6f75726e616c73",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.quit": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e71756974",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.quitting": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e7175697474696e67",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.relay.setup": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e72656c61792e7365747570",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.remove": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e72656d6f7665",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.remove.body": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e72656d6f76652e626f6479",
    formatIntent: "text",
    arguments: [
      { name: "name", type: "string", min: null, max: null, numericStyle: "ungrouped" },
    ],
  },
  "journal.windows.remove.failed": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e72656d6f76652e6661696c6564",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.remove.title": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e72656d6f76652e7469746c65",
    formatIntent: "text",
    arguments: [
      { name: "name", type: "string", min: null, max: null, numericStyle: "ungrouped" },
    ],
  },
  "journal.windows.restart": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e72657374617274",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.run.state.link": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e72756e2e73746174652e6c696e6b",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.setup.app.term": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e73657475702e6170702e7465726d",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.setup.app.text": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e73657475702e6170702e74657874",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.setup.band": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e73657475702e62616e64",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.setup.fits.title": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e73657475702e666974732e7469746c65",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.setup.folder.label": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e73657475702e666f6c6465722e6c6162656c",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.setup.folder.text": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e73657475702e666f6c6465722e74657874",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.setup.folder.title": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e73657475702e666f6c6465722e7469746c65",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.setup.journal.term": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e73657475702e6a6f75726e616c2e7465726d",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.setup.journal.text": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e73657475702e6a6f75726e616c2e74657874",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.setup.local.desc": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e73657475702e6c6f63616c2e64657363",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.setup.on.this.pc": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e73657475702e6f6e2e746869732e7063",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.setup.sign.in": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e73657475702e7369676e2e696e",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.setup.start": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e73657475702e7374617274",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.setup.step": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e73657475702e73746570",
    formatIntent: "text",
    arguments: [
      { name: "n", type: "string", min: null, max: null, numericStyle: "ungrouped" },
    ],
  },
  "journal.windows.setup.stopped": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e73657475702e73746f70706564",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.setup.subtitle": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e73657475702e7375627469746c65",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.setup.thinking.lead": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e73657475702e7468696e6b696e672e6c656164",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.setup.thinking.term": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e73657475702e7468696e6b696e672e7465726d",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.setup.thinking.text": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e73657475702e7468696e6b696e672e74657874",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.setup.thinking.title": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e73657475702e7468696e6b696e672e7469746c65",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.setup.title": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e73657475702e7469746c65",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.show.in.explorer": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e73686f772e696e2e6578706c6f726572",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.signin.failed": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e7369676e696e2e6661696c6564",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.size.bytes": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e73697a652e6279746573",
    formatIntent: "cardinal",
    selectorVar: "quantity",
    arguments: [
      { name: "quantity", type: "cardinal", min: null, max: null, numericStyle: "ungrouped" },
    ],
  },
  "journal.windows.size.gb": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e73697a652e6762",
    formatIntent: "text",
    arguments: [
      { name: "value", type: "string", min: null, max: null, numericStyle: "ungrouped" },
    ],
  },
  "journal.windows.size.kb": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e73697a652e6b62",
    formatIntent: "text",
    arguments: [
      { name: "value", type: "string", min: null, max: null, numericStyle: "ungrouped" },
    ],
  },
  "journal.windows.size.mb": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e73697a652e6d62",
    formatIntent: "text",
    arguments: [
      { name: "value", type: "string", min: null, max: null, numericStyle: "ungrouped" },
    ],
  },
  "journal.windows.size.tb": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e73697a652e7462",
    formatIntent: "text",
    arguments: [
      { name: "value", type: "string", min: null, max: null, numericStyle: "ungrouped" },
    ],
  },
  "journal.windows.start": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e7374617274",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.start.at.sign.in": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e73746172742e61742e7369676e2e696e",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.startup.title": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e737461727475702e7469746c65",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.status.failed": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e7374617475732e6661696c6564",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.steps.brain": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e73746570732e627261696e",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.steps.doctor": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e73746570732e646f63746f72",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.steps.install.models": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e73746570732e696e7374616c6c2e6d6f64656c73",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.steps.journal": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e73746570732e6a6f75726e616c",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.steps.service": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e73746570732e73657276696365",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.stop": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e73746f70",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.title": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e7469746c65",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.try.again": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e7472792e616761696e",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.unknown": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e756e6b6e6f776e",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.unnamed.device": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e756e6e616d65642e646576696365",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.updates.auto.check": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e757064617465732e6175746f2e636865636b",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.updates.auto.download": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e757064617465732e6175746f2e646f776e6c6f6164",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.updates.automatic": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e757064617465732e6175746f6d61746963",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.updates.available.subtitle": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e757064617465732e617661696c61626c652e7375627469746c65",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.updates.available.title": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e757064617465732e617661696c61626c652e7469746c65",
    formatIntent: "text",
    arguments: [
      { name: "target", type: "string", min: null, max: null, numericStyle: "ungrouped" },
    ],
  },
  "journal.windows.updates.check.again": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e757064617465732e636865636b2e616761696e",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.updates.check.now": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e757064617465732e636865636b2e6e6f77",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.updates.checking": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e757064617465732e636865636b696e67",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.updates.checking.with.downloaded": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e757064617465732e636865636b696e672e776974682e646f776e6c6f61646564",
    formatIntent: "text",
    arguments: [
      { name: "downloaded", type: "string", min: null, max: null, numericStyle: "ungrouped" },
    ],
  },
  "journal.windows.updates.day": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e757064617465732e646179",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.updates.deferral.metered": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e757064617465732e646566657272616c2e6d657465726564",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.updates.deferral.restricted": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e757064617465732e646566657272616c2e72657374726963746564",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.updates.deferral.roaming": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e757064617465732e646566657272616c2e726f616d696e67",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.updates.deferral.unknown": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e757064617465732e646566657272616c2e756e6b6e6f776e",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.updates.download": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e757064617465732e646f776e6c6f6164",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.updates.downloading.retains.downloaded": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e757064617465732e646f776e6c6f6164696e672e72657461696e732e646f776e6c6f61646564",
    formatIntent: "text",
    arguments: [
      { name: "downloaded", type: "string", min: null, max: null, numericStyle: "ungrouped" },
    ],
  },
  "journal.windows.updates.downloading.title": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e757064617465732e646f776e6c6f6164696e672e7469746c65",
    formatIntent: "text",
    arguments: [
      { name: "target", type: "string", min: null, max: null, numericStyle: "ungrouped" },
    ],
  },
  "journal.windows.updates.failure.check.body": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e757064617465732e6661696c7572652e636865636b2e626f6479",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.updates.failure.check.retained.body": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e757064617465732e6661696c7572652e636865636b2e72657461696e65642e626f6479",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.updates.failure.check.title": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e757064617465732e6661696c7572652e636865636b2e7469746c65",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.updates.failure.download.body": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e757064617465732e6661696c7572652e646f776e6c6f61642e626f6479",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.updates.failure.download.retained.body": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e757064617465732e6661696c7572652e646f776e6c6f61642e72657461696e65642e626f6479",
    formatIntent: "text",
    arguments: [
      { name: "downloaded", type: "string", min: null, max: null, numericStyle: "ungrouped" },
    ],
  },
  "journal.windows.updates.failure.download.title": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e757064617465732e6661696c7572652e646f776e6c6f61642e7469746c65",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.updates.failure.install.body": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e757064617465732e6661696c7572652e696e7374616c6c2e626f6479",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.updates.failure.install.missing": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e757064617465732e6661696c7572652e696e7374616c6c2e6d697373696e67",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.updates.failure.install.title": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e757064617465732e6661696c7572652e696e7374616c6c2e7469746c65",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.updates.failure.stop.body": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e757064617465732e6661696c7572652e73746f702e626f6479",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.updates.failure.stop.title": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e757064617465732e6661696c7572652e73746f702e7469746c65",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.updates.header": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e757064617465732e686561646572",
    formatIntent: "text",
    arguments: [
      { name: "version", type: "string", min: null, max: null, numericStyle: "ungrouped" },
    ],
  },
  "journal.windows.updates.how.often": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e757064617465732e686f772e6f6674656e",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.updates.install": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e757064617465732e696e7374616c6c",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.updates.installing.subtitle": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e757064617465732e696e7374616c6c696e672e7375627469746c65",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.updates.installing.title": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e757064617465732e696e7374616c6c696e672e7469746c65",
    formatIntent: "text",
    arguments: [
      { name: "downloaded", type: "string", min: null, max: null, numericStyle: "ungrouped" },
    ],
  },
  "journal.windows.updates.just.now": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e757064617465732e6a7573742e6e6f77",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.updates.last.checked": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e757064617465732e6c6173742e636865636b6564",
    formatIntent: "text",
    arguments: [
      { name: "relative", type: "string", min: null, max: null, numericStyle: "ungrouped" },
    ],
  },
  "journal.windows.updates.month": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e757064617465732e6d6f6e7468",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.updates.never.checked": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e757064617465732e6e657665722e636865636b6564",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.updates.privacy": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e757064617465732e70726976616379",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.updates.ready.title": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e757064617465732e72656164792e7469746c65",
    formatIntent: "text",
    arguments: [
      { name: "downloaded", type: "string", min: null, max: null, numericStyle: "ungrouped" },
    ],
  },
  "journal.windows.updates.retry.check": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e757064617465732e72657472792e636865636b",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.updates.retry.download": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e757064617465732e72657472792e646f776e6c6f6164",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.updates.unavailable.subtitle": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e757064617465732e756e617661696c61626c652e7375627469746c65",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.updates.unavailable.title": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e757064617465732e756e617661696c61626c652e7469746c65",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.updates.up.to.date": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e757064617465732e75702e746f2e64617465",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.updates.week": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e757064617465732e7765656b",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.what.happened": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e776861742e68617070656e6564",
    formatIntent: "text",
    arguments: [
    ],
  },
  "journal.windows.your.devices": {
    resourceKey: "m6a6f75726e616c2e77696e646f77732e796f75722e64657669636573",
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
globalThis.SolstoneI18nBundles["journal-windows-app"] = Object.freeze({render, integer, cardinal, messageIds: Object.freeze(Object.keys(MESSAGES))});
})();
