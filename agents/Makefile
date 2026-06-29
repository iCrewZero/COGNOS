# COGNOS/OS — Python agent build helpers
# Owner: iCrewZero

.PHONY: proto test clean

proto:
	python generate_proto.py

test:
	pytest ../tests/ -v --tb=short

clean:
	rm -f cognos_pb2.py cognos_pb2_grpc.py cognos_pb2*_pb2.py
