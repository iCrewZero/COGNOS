# COGNOS/OS — Python agent build helpers
# Owner: iCrewZero

PYTHON ?= python3

.PHONY: proto test clean

proto:
	$(PYTHON) generate_proto.py

test:
	pytest tests/ -v --tb=short

clean:
	rm -f proto/cognos_pb2.py proto/cognos_pb2_grpc.py
	rm -rf proto/__pycache__
