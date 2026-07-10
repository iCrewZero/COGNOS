#!/usr/bin/env python3
import inspect
from vllm.sampling_params import StructuredOutputsParams, SamplingParams
print("StructuredOutputsParams:", StructuredOutputsParams)
try:
    print("sig:", inspect.signature(StructuredOutputsParams))
except Exception as e:
    print("err", e)
# try construction
for kwargs in [
    {"json": "{}"},
    {"json_schema": "{}"},
    {"schema": "{}"},
]:
    try:
        p = StructuredOutputsParams(**kwargs)
        print("ok", kwargs, p)
    except Exception as e:
        print("fail", kwargs, e)
