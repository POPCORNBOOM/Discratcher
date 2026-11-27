import type { InjectionKey, Ref } from 'vue'

/** The open section's name, shared by a `UiAccordion` with the `UiSection`s inside it. */
export const ACCORDION: InjectionKey<Ref<string>> = Symbol('accordion')
