#!/bin/sh

set -x

# Check for "Ready" nodes
if [ -z "$(kubectl get nodes -o jsonpath='{.items[?(@.status.conditions[-1].type=="Ready")].metadata.name}')" ]; then
	rm -f /output/operator.yaml /output/gateway.yaml
	exit 1
fi

# Identify first run.
# - Apply CRDs if not present
# - Create operator kubeconfig with correct server address
# - Create gateway kubeconfig with correct server address
if ! kubectl get crd hosts.runtime.wasmcloud.dev 2>&1 >/dev/null; then
	kubectl apply -f /crds/

	cp /output/kubeconfig.yaml /output/in-docker.yaml
	KUBECONFIG=/output/in-docker.yaml kubectl config set-cluster default --server=https://kubernetes:6443

	cp /output/in-docker.yaml /output/operator.yaml
	cp /output/in-docker.yaml /output/gateway.yaml
fi

exit 0
