import { FluentResource } from "./resource.js";
import { FluentValue, FluentFunction } from "./types.js";
import { Message, Term, Pattern } from "./ast.js";
import { IntlCache } from "./memoizer.js";
export type TextTransform = (text: string) => string;
export type FluentVariable = FluentValue | string | number | Date;
/**
 * Message bundles are single-language stores of translation resources. They are
 * responsible for formatting message values and attributes to strings.
 */
export declare class FluentBundle {
    locales: Array<string>;
    /** @ignore */
    _terms: Map<string, Term>;
    /** @ignore */
    _messages: Map<string, Message>;
    /** @ignore */
    _functions: Record<string, FluentFunction>;
    /** @ignore */
    _useIsolating: boolean;
    /** @ignore */
    _transform: TextTransform;
    /** @ignore */
    _intls: IntlCache;
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
    constructor(locales: string | Array<string>, { functions, useIsolating, transform, }?: {
        /** Additional functions available to translations as builtins. */
        functions?: Record<string, FluentFunction>;
        /**
         * Whether to use Unicode isolation marks (FSI, PDI) for bidi interpolations.
         *
         * Default: `true`.
         */
        useIsolating?: boolean;
        /** A function used to transform string parts of patterns. */
        transform?: TextTransform;
    });
    /**
     * Check if a message is present in the bundle.
     *
     * @param id - The identifier of the message to check.
     */
    hasMessage(id: string): boolean;
    /**
     * Return a raw unformatted message object from the bundle.
     *
     * Raw messages are `{value, attributes}` shapes containing translation units
     * called `Patterns`. `Patterns` are implementation-specific; they should be
     * treated as black boxes and formatted with `FluentBundle.formatPattern`.
     *
     * @param id - The identifier of the message to check.
     */
    getMessage(id: string): Message | undefined;
    /**
     * Add a translation resource to the bundle.
     *
     * @example
     * ```js
     * let res = new FluentResource("foo = Foo");
     * bundle.addResource(res);
     * bundle.getMessage("foo");
     * // → {value: .., attributes: {..}}
     * ```
     *
     * @param res
     * @param options
     */
    addResource(res: FluentResource, { allowOverrides, }?: {
        /**
         * Boolean specifying whether it's allowed to override
         * an existing message or term with a new value.
         *
         * Default: `false`.
         */
        allowOverrides?: boolean;
    }): Array<Error>;
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
    formatPattern(pattern: Pattern, args?: Record<string, FluentVariable> | null, errors?: Array<Error> | null): string;
}
