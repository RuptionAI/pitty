
export function start() {
wasm.start();
}
function __wbg_get_imports() {
            const import0 = {
            __proto__: null,
        __wbg___wbindgen_is_function_fcda5e3902d732fe: function(arg0) {
const ret = typeof(getObject(arg0)) === 'function';
return ret;
},
__wbg___wbindgen_is_undefined_8c687d0b90d5b524: function(arg0) {
const ret = getObject(arg0) === undefined;
return ret;
},
__wbg___wbindgen_number_get_1dc732b810cb937c: function(arg0, arg1) {
const obj = getObject(arg1);
const ret = typeof(obj) === 'number' ? obj : undefined;
getDataViewMemory0().setFloat64(arg0 + 8 * 1, isLikeNone(ret) ? 0 : ret, true);
getDataViewMemory0().setInt32(arg0 + 4 * 0, !isLikeNone(ret), true);
},
__wbg___wbindgen_rethrow_dba7bb2caa14ba21: function(arg0) {
throw takeObject(arg0);
},
__wbg___wbindgen_throw_5d9e815e6fdf150f: function(arg0, arg1) {
throw new Error(getStringFromWasm0(arg0, arg1));
},
__wbg__wbg_cb_unref_997e73d32238e655: function(arg0) {
getObject(arg0)._wbg_cb_unref();
},
__wbg_addEventListener_9534e2dc12044ee4: function() { return handleError(function (arg0, arg1, arg2, arg3) {
getObject(arg0).addEventListener(getStringFromWasm0(arg1, arg2), getObject(arg3));
}, arguments); },
__wbg_addRange_6a48fe41e98f1d98: function() { return handleError(function (arg0, arg1) {
getObject(arg0).addRange(getObject(arg1));
}, arguments); },
__wbg_add_8ead3105ef6d0bda: function() { return handleError(function (arg0, arg1, arg2) {
getObject(arg0).add(getStringFromWasm0(arg1, arg2));
}, arguments); },
__wbg_altKey_fa381deee4b395cd: function(arg0) {
const ret = getObject(arg0).altKey;
return ret;
},
__wbg_bottom_89fa307d0439be2c: function(arg0) {
const ret = getObject(arg0).bottom;
return ret;
},
__wbg_call_6bcf8d3e20937e46: function() { return handleError(function (arg0, arg1, arg2) {
const ret = getObject(arg0).call(getObject(arg1), getObject(arg2));
return addHeapObject(ret);
}, arguments); },
__wbg_classList_1501fdb96a8c296c: function(arg0) {
const ret = getObject(arg0).classList;
return addHeapObject(ret);
},
__wbg_createRange_b018c82ba7997cf4: function() { return handleError(function (arg0) {
const ret = getObject(arg0).createRange();
return addHeapObject(ret);
}, arguments); },
__wbg_ctrlKey_785e802b8798af13: function(arg0) {
const ret = getObject(arg0).ctrlKey;
return ret;
},
__wbg_defaultPrevented_6532d5b02b675eeb: function(arg0) {
const ret = getObject(arg0).defaultPrevented;
return ret;
},
__wbg_document_c7f486c52d63d24e: function(arg0) {
const ret = getObject(arg0).document;
return isLikeNone(ret) ? 0 : addHeapObject(ret);
},
__wbg_getAttribute_b12f8535a2cb5368: function(arg0, arg1, arg2, arg3) {
const ret = getObject(arg1).getAttribute(getStringFromWasm0(arg2, arg3));
var ptr1 = isLikeNone(ret) ? 0 : passStringToWasm0(ret, wasm.__wbindgen_export2, wasm.__wbindgen_export3);
var len1 = WASM_VECTOR_LEN;
getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
},
__wbg_getBoundingClientRect_299f40f8641457e6: function(arg0) {
const ret = getObject(arg0).getBoundingClientRect();
return addHeapObject(ret);
},
__wbg_getElementById_ccc92d66acf76819: function(arg0, arg1, arg2) {
const ret = getObject(arg0).getElementById(getStringFromWasm0(arg1, arg2));
return isLikeNone(ret) ? 0 : addHeapObject(ret);
},
__wbg_getSelection_2ba8001410fa1223: function() { return handleError(function (arg0) {
const ret = getObject(arg0).getSelection();
return isLikeNone(ret) ? 0 : addHeapObject(ret);
}, arguments); },
__wbg_get_989d0a1309644f2b: function() { return handleError(function (arg0, arg1) {
const ret = Reflect.get(getObject(arg0), getObject(arg1));
return addHeapObject(ret);
}, arguments); },
__wbg_innerHeight_f0945dfd6f2d3a87: function() { return handleError(function (arg0) {
const ret = getObject(arg0).innerHeight;
return addHeapObject(ret);
}, arguments); },
__wbg_instanceof_Element_c95c45635d03047e: function(arg0) {
let result;
                    try {
                        result = getObject(arg0) instanceof Element;
                    } catch (_) {
                        result = false;
                    }
                    const ret = result;
return ret;
},
__wbg_instanceof_HtmlButtonElement_5a447ec65f8a3786: function(arg0) {
let result;
                    try {
                        result = getObject(arg0) instanceof HTMLButtonElement;
                    } catch (_) {
                        result = false;
                    }
                    const ret = result;
return ret;
},
__wbg_instanceof_HtmlElement_bd89ce6abf9dad44: function(arg0) {
let result;
                    try {
                        result = getObject(arg0) instanceof HTMLElement;
                    } catch (_) {
                        result = false;
                    }
                    const ret = result;
return ret;
},
__wbg_instanceof_HtmlInputElement_086894dc9c868a64: function(arg0) {
let result;
                    try {
                        result = getObject(arg0) instanceof HTMLInputElement;
                    } catch (_) {
                        result = false;
                    }
                    const ret = result;
return ret;
},
__wbg_instanceof_Promise_f6320f682f582ddf: function(arg0) {
let result;
                    try {
                        result = getObject(arg0) instanceof Promise;
                    } catch (_) {
                        result = false;
                    }
                    const ret = result;
return ret;
},
__wbg_instanceof_Window_a3b8566f0a9c5d1a: function(arg0) {
let result;
                    try {
                        result = getObject(arg0) instanceof Window;
                    } catch (_) {
                        result = false;
                    }
                    const ret = result;
return ret;
},
__wbg_isContentEditable_ce3dc9cb9478a7a4: function(arg0) {
const ret = getObject(arg0).isContentEditable;
return ret;
},
__wbg_item_adf6c9a98cc42542: function(arg0, arg1) {
const ret = getObject(arg0).item(arg1 >>> 0);
return isLikeNone(ret) ? 0 : addHeapObject(ret);
},
__wbg_key_d29d670923b81510: function(arg0, arg1) {
const ret = getObject(arg1).key;
const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_export2, wasm.__wbindgen_export3);
const len1 = WASM_VECTOR_LEN;
getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
},
__wbg_length_2a004c1681a133a1: function(arg0) {
const ret = getObject(arg0).length;
return ret;
},
__wbg_metaKey_3f9183dfd5e1b726: function(arg0) {
const ret = getObject(arg0).metaKey;
return ret;
},
__wbg_name_18d155d853ac8ed4: function(arg0, arg1) {
const ret = getObject(arg1).name;
const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_export2, wasm.__wbindgen_export3);
const len1 = WASM_VECTOR_LEN;
getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
},
__wbg_navigator_d217ca64c4bbff48: function(arg0) {
const ret = getObject(arg0).navigator;
return addHeapObject(ret);
},
__wbg_preventDefault_4235a4ccf8540533: function(arg0) {
getObject(arg0).preventDefault();
},
__wbg_querySelectorAll_fdbf93e11103921d: function() { return handleError(function (arg0, arg1, arg2) {
const ret = getObject(arg0).querySelectorAll(getStringFromWasm0(arg1, arg2));
return addHeapObject(ret);
}, arguments); },
__wbg_querySelector_5b1d9ebad373dc9d: function() { return handleError(function (arg0, arg1, arg2) {
const ret = getObject(arg0).querySelector(getStringFromWasm0(arg1, arg2));
return isLikeNone(ret) ? 0 : addHeapObject(ret);
}, arguments); },
__wbg_querySelector_e621a716a5ee2aa9: function() { return handleError(function (arg0, arg1, arg2) {
const ret = getObject(arg0).querySelector(getStringFromWasm0(arg1, arg2));
return isLikeNone(ret) ? 0 : addHeapObject(ret);
}, arguments); },
__wbg_queueMicrotask_85c90f6987555d65: function(arg0) {
const ret = getObject(arg0).queueMicrotask;
return addHeapObject(ret);
},
__wbg_queueMicrotask_f6a1fa10b81d1fc0: function(arg0) {
queueMicrotask(getObject(arg0));
},
__wbg_removeAllRanges_c662e8bd4099f2cd: function() { return handleError(function (arg0) {
getObject(arg0).removeAllRanges();
}, arguments); },
__wbg_remove_10865d1ab6497abe: function() { return handleError(function (arg0, arg1, arg2) {
getObject(arg0).remove(getStringFromWasm0(arg1, arg2));
}, arguments); },
__wbg_repeat_01e85ccb1bcef326: function(arg0) {
const ret = getObject(arg0).repeat;
return ret;
},
__wbg_resolve_35ec7e0c6af4c82c: function(arg0) {
const ret = Promise.resolve(getObject(arg0));
return addHeapObject(ret);
},
__wbg_selectNodeContents_b247cae11d9bf050: function() { return handleError(function (arg0, arg1) {
getObject(arg0).selectNodeContents(getObject(arg1));
}, arguments); },
__wbg_setAttribute_8c84e9351986b1f6: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4) {
getObject(arg0).setAttribute(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4));
}, arguments); },
__wbg_setTimeout_7b1dfbeaccbf9bb6: function() { return handleError(function (arg0, arg1, arg2) {
const ret = getObject(arg0).setTimeout(getObject(arg1), arg2);
return ret;
}, arguments); },
__wbg_set_checked_d4ef002a3dc5d2e4: function(arg0, arg1) {
getObject(arg0).checked = arg1 !== 0;
},
__wbg_set_disabled_9deb3e27f9458f9c: function(arg0, arg1) {
getObject(arg0).disabled = arg1 !== 0;
},
__wbg_set_textContent_729c78ca859aa5a5: function(arg0, arg1, arg2) {
getObject(arg0).textContent = arg1 === 0 ? undefined : getStringFromWasm0(arg1, arg2);
},
__wbg_set_type_a6be0ef93bdae5b8: function(arg0, arg1, arg2) {
getObject(arg0).type = getStringFromWasm0(arg1, arg2);
},
__wbg_static_accessor_GLOBAL_8eb4cd83130a11a0: function() {
const ret = typeof global === 'undefined' ? null : global;
return isLikeNone(ret) ? 0 : addHeapObject(ret);
},
__wbg_static_accessor_GLOBAL_THIS_1e7044f654e934db: function() {
const ret = typeof globalThis === 'undefined' ? null : globalThis;
return isLikeNone(ret) ? 0 : addHeapObject(ret);
},
__wbg_static_accessor_SELF_d8b50611246a6d92: function() {
const ret = typeof self === 'undefined' ? null : self;
return isLikeNone(ret) ? 0 : addHeapObject(ret);
},
__wbg_static_accessor_WINDOW_fd0bc376bf0f8b42: function() {
const ret = typeof window === 'undefined' ? null : window;
return isLikeNone(ret) ? 0 : addHeapObject(ret);
},
__wbg_tagName_f764922f7d763584: function(arg0, arg1) {
const ret = getObject(arg1).tagName;
const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_export2, wasm.__wbindgen_export3);
const len1 = WASM_VECTOR_LEN;
getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
},
__wbg_target_3e80dc66a5936e87: function(arg0) {
const ret = getObject(arg0).target;
return isLikeNone(ret) ? 0 : addHeapObject(ret);
},
__wbg_textContent_d7e4a707af82657f: function(arg0, arg1) {
const ret = getObject(arg1).textContent;
var ptr1 = isLikeNone(ret) ? 0 : passStringToWasm0(ret, wasm.__wbindgen_export2, wasm.__wbindgen_export3);
var len1 = WASM_VECTOR_LEN;
getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
},
__wbg_then_7a850dae4493f353: function(arg0, arg1, arg2) {
const ret = getObject(arg0).then(getObject(arg1), getObject(arg2));
return addHeapObject(ret);
},
__wbg_then_b830475380919203: function(arg0, arg1) {
const ret = getObject(arg0).then(getObject(arg1));
return addHeapObject(ret);
},
__wbg_top_319e8c7a40daacf1: function(arg0) {
const ret = getObject(arg0).top;
return ret;
},
__wbg_type_e9e1e62b9a2c4682: function(arg0, arg1) {
const ret = getObject(arg1).type;
const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_export2, wasm.__wbindgen_export3);
const len1 = WASM_VECTOR_LEN;
getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
},
__wbg_value_8215ae6d5fec3d34: function(arg0, arg1) {
const ret = getObject(arg1).value;
const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_export2, wasm.__wbindgen_export3);
const len1 = WASM_VECTOR_LEN;
getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
},
__wbindgen_generic_0000000000000001: function(arg0, arg1) {
// Cast intrinsic for `Closure(Closure { owned: true, function: Function { arguments: [Externref], shim_idx: 14, ret: Result(Unit), inner_ret: Some(Result(Unit)) }, mutable: true }) -> Externref`.
const ret = makeMutClosure(arg0, arg1, __wasm_bindgen_func_elem_182);
return addHeapObject(ret);
},
__wbindgen_generic_0000000000000002: function(arg0, arg1) {
// Cast intrinsic for `Closure(Closure { owned: true, function: Function { arguments: [NamedExternref("Event")], shim_idx: 1, ret: Unit, inner_ret: Some(Unit) }, mutable: true }) -> Externref`.
const ret = makeMutClosure(arg0, arg1, __wasm_bindgen_func_elem_88);
return addHeapObject(ret);
},
__wbindgen_generic_0000000000000003: function(arg0, arg1) {
// Cast intrinsic for `Closure(Closure { owned: true, function: Function { arguments: [NamedExternref("KeyboardEvent")], shim_idx: 1, ret: Unit, inner_ret: Some(Unit) }, mutable: true }) -> Externref`.
const ret = makeMutClosure(arg0, arg1, __wasm_bindgen_func_elem_88_3);
return addHeapObject(ret);
},
__wbindgen_generic_0000000000000004: function(arg0, arg1) {
// Cast intrinsic for `Closure(Closure { owned: true, function: Function { arguments: [], shim_idx: 4, ret: Unit, inner_ret: Some(Unit) }, mutable: true }) -> Externref`.
const ret = makeMutClosure(arg0, arg1, __wasm_bindgen_func_elem_93);
return addHeapObject(ret);
},
__wbindgen_generic_0000000000000005: function(arg0, arg1) {
// Cast intrinsic for `Ref(String) -> Externref`.
const ret = getStringFromWasm0(arg0, arg1);
return addHeapObject(ret);
},
__wbindgen_object_clone_ref: function(arg0) {
const ret = getObject(arg0);
return addHeapObject(ret);
},
__wbindgen_object_drop_ref: function(arg0) {
takeObject(arg0);
},
};
        return {
            __proto__: null,
            "./browser_bg.js": import0,
        };
}

function __wasm_bindgen_func_elem_93(arg0, arg1) {
wasm.__wasm_bindgen_func_elem_93(arg0, arg1);
}

function __wasm_bindgen_func_elem_88(arg0, arg1, arg2) {
wasm.__wasm_bindgen_func_elem_88(arg0, arg1, addHeapObject(arg2));
}

function __wasm_bindgen_func_elem_88_3(arg0, arg1, arg2) {
wasm.__wasm_bindgen_func_elem_88_3(arg0, arg1, addHeapObject(arg2));
}

function __wasm_bindgen_func_elem_182(arg0, arg1, arg2) {
try {
const retptr = wasm.__wbindgen_add_to_stack_pointer(-16);
wasm.__wasm_bindgen_func_elem_182(retptr, arg0, arg1, addHeapObject(arg2));
var r0 = getDataViewMemory0().getInt32(retptr + 4 * 0, true);
var r1 = getDataViewMemory0().getInt32(retptr + 4 * 1, true);
if (r1) {
throw takeObject(r0);
}
} finally {
wasm.__wbindgen_add_to_stack_pointer(16);
}
}

function addHeapObject(obj) {
                    if (heap_next === heap.length) heap.push(heap.length + 1);
                    const idx = heap_next;
                    heap_next = heap[idx];
                    
                    heap[idx] = obj;
                    return idx;
                }

const CLOSURE_DTORS = (typeof FinalizationRegistry === 'undefined')
                        ? { register: () => {}, unregister: () => {} }
                        : new FinalizationRegistry(state => wasm.__wbindgen_export4(state.a, state.b));

function dropObject(idx) {
                    if (idx < 1028) return;
                    heap[idx] = heap_next;
                    heap_next = idx;
                }

let cachedDataViewMemory0 = null;
                function getDataViewMemory0() {
                    if (cachedDataViewMemory0 === null || cachedDataViewMemory0.buffer.detached === true || (cachedDataViewMemory0.buffer.detached === undefined && cachedDataViewMemory0.buffer !== wasm.memory.buffer)) {
                        cachedDataViewMemory0 = new DataView(wasm.memory.buffer);
                    }
                    return cachedDataViewMemory0;
                }

function getStringFromWasm0(ptr, len) {
                    return decodeText(ptr >>> 0, len);
                }

let cachedUint8ArrayMemory0 = null;
                function getUint8ArrayMemory0() {
                    if (cachedUint8ArrayMemory0 === null || cachedUint8ArrayMemory0.byteLength === 0) {
                        cachedUint8ArrayMemory0 = new Uint8Array(wasm.memory.buffer);
                    }
                    return cachedUint8ArrayMemory0;
                }

function getObject(idx) { return heap[idx]; }

function handleError(f, args) {
                            try {
                                return f.apply(this, args);
                            } catch (e) {
                                wasm.__wbindgen_export(addHeapObject(e));
                            }
                        }

let heap = new Array(1024).fill(undefined);
                heap.push(undefined, null, true, false);

let heap_next = heap.length;

function isLikeNone(x) {
                return x === undefined || x === null;
            }

function makeMutClosure(arg0, arg1, f) {
                    const state = { a: arg0, b: arg1, cnt: 1 };
                    const real = (...args) => {
                        
                        // First up with a closure we increment the internal reference
                        // count. This ensures that the Rust closure environment won't
                        // be deallocated while we're invoking it.
                        state.cnt++;
                        const a = state.a;
                        state.a = 0;
                        try {
                            return f(a, state.b, ...args);
                        } finally {
                            state.a = a;
                            real._wbg_cb_unref();
                        }
                    };
                    real._wbg_cb_unref = () => {
                        if (--state.cnt === 0) {
                            wasm.__wbindgen_export4(state.a, state.b);
                            state.a = 0;
                            CLOSURE_DTORS.unregister(state);
                        }
                    };
                    CLOSURE_DTORS.register(real, state, state);
                    return real;
                }

function passStringToWasm0(arg, malloc, realloc) {
                    if (realloc === undefined) {
                    const buf = cachedTextEncoder.encode(arg);
                    const ptr = malloc(buf.length, 1) >>> 0;
                    getUint8ArrayMemory0().subarray(ptr, ptr + buf.length).set(buf);
                    WASM_VECTOR_LEN = buf.length;
                    return ptr;
                }

                let len = arg.length;
                let ptr = malloc(len, 1) >>> 0;

                const mem = getUint8ArrayMemory0();

                let offset = 0;

                for (; offset < len; offset++) {
                    const code = arg.charCodeAt(offset);
                    if (code > 0x7F) break;
                    mem[ptr + offset] = code;
                }
            if (offset !== len) {
                        if (offset !== 0) {
                            arg = arg.slice(offset);
                        }
                        ptr = realloc(ptr, len, len = offset + arg.length * 3, 1) >>> 0;
                        const view = getUint8ArrayMemory0().subarray(ptr + offset, ptr + len);
                        const ret = cachedTextEncoder.encodeInto(arg, view);
                        
                        offset += ret.written;
                        ptr = realloc(ptr, len, offset, 1) >>> 0;
                    }

                    WASM_VECTOR_LEN = offset;
                    return ptr;
                }

function takeObject(idx) {
                const ret = getObject(idx);
                dropObject(idx);
                return ret;
            }

let cachedTextDecoder = new TextDecoder('utf-8', { ignoreBOM: true, fatal: true });
cachedTextDecoder.decode();
                        const MAX_SAFARI_DECODE_BYTES = 2146435072;
                        let numBytesDecoded = 0;
                        function decodeText(ptr, len) {
                            numBytesDecoded += len;
                            if (numBytesDecoded >= MAX_SAFARI_DECODE_BYTES) {
                                cachedTextDecoder = new TextDecoder('utf-8', { ignoreBOM: true, fatal: true });
                                cachedTextDecoder.decode();
                                numBytesDecoded = len;
                            }
                            return cachedTextDecoder.decode(getUint8ArrayMemory0().subarray(ptr, ptr + len));
                        }

const cachedTextEncoder = new TextEncoder();

                        if (!('encodeInto' in cachedTextEncoder)) {
                            cachedTextEncoder.encodeInto = function (arg, view) {
                const buf = cachedTextEncoder.encode(arg);
                view.set(buf);
                return {
                    read: arg.length,
                    written: buf.length
                };
            };
                        }

let WASM_VECTOR_LEN = 0;

let wasmModule, wasmInstance, wasm;
            function __wbg_finalize_init(instance, module) {
                wasmInstance = instance;
                wasm = instance.exports;
                wasmModule = module;
                cachedDataViewMemory0 = null;
cachedUint8ArrayMemory0 = null;
wasm.__wbindgen_start();
return wasm;
            }

            async function __wbg_load(module, imports) {
                if (typeof Response === 'function' && module instanceof Response) {
                    if (!module.ok) {
                        throw new Error(`failed to fetch Wasm: ${module.status} ${module.statusText} fetching '${module.url}'`);
                    }

                    if (typeof WebAssembly.instantiateStreaming === 'function') {
                        try {
                            return await WebAssembly.instantiateStreaming(module, imports);
                        } catch (e) {
                            const validResponse = expectedResponseType(module.type);

                            if (validResponse && module.headers.get('Content-Type') !== 'application/wasm') {
                                console.warn("`WebAssembly.instantiateStreaming` failed because your server does not serve Wasm with `application/wasm` MIME type. Falling back to `WebAssembly.instantiate` which is slower. Original error:\n", e);

                            } else { throw e; }
                        }
                    }

                    const bytes = await module.arrayBuffer();
                    return await WebAssembly.instantiate(bytes, imports);
                } else {
                    const instance = await WebAssembly.instantiate(module, imports);

                    if (instance instanceof WebAssembly.Instance) {
                        return { instance, module };
                    } else {
                        return instance;
                    }
                }

                function expectedResponseType(type) {
                    switch (type) {
                        case 'basic': case 'cors': case 'default': return true;
                    }
                    return false;
                }
            }

            function initSync(module) {
                if (wasm !== undefined) return wasm;

                
                if (module !== undefined) {
                    if (Object.getPrototypeOf(module) === Object.prototype) {
                        ({module} = module)
                    } else {
                        console.warn('using deprecated parameters for `initSync()`; pass a single object instead')
                    }
                }

                const imports = __wbg_get_imports();
                if (!(module instanceof WebAssembly.Module)) {
                    module = new WebAssembly.Module(module);
                }
                const instance = new WebAssembly.Instance(module, imports);
                return __wbg_finalize_init(instance, module);
            }

            async function __wbg_init(module_or_path) {
                if (wasm !== undefined) return wasm;

                
                if (module_or_path !== undefined) {
                    if (Object.getPrototypeOf(module_or_path) === Object.prototype) {
                        ({module_or_path} = module_or_path)
                    } else {
                        console.warn('using deprecated parameters for the initialization function; pass a single object instead')
                    }
                }

                if (module_or_path === undefined) {
                            module_or_path = new URL('browser.68ae2823aebf.wasm', import.meta.url);
                        }
                const imports = __wbg_get_imports();

                if (typeof module_or_path === 'string' || (typeof Request === 'function' && module_or_path instanceof Request) || (typeof URL === 'function' && module_or_path instanceof URL)) {
                    module_or_path = fetch(module_or_path);
                }

                const { instance, module } = await __wbg_load(await module_or_path, imports);

                return __wbg_finalize_init(instance, module);
            }
            
export { initSync, __wbg_init as default };
__wbg_init();
