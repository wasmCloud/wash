### To Deploy locally

Start kind cluster

```sh
go tool kind create cluster
```

**Install the CRDs into the cluster:**

```sh
make install
```

**Run the Manager:**

```sh
make devlog
```

You can apply the samples (examples) from the config/sample:

```sh
kubectl apply -k config/samples/dev
```

### To Deploy on remote cluster

**Build and push your image to the location specified by `IMG`:**

```sh
make docker-build docker-push IMG=<some-registry>/operator:tag
```

**NOTE:** This image ought to be published in the personal registry you specified.
And it is required to have access to pull the image from the working environment.
Make sure you have the proper permission to the registry if the above commands don’t work.

**Install the CRDs into the cluster:**

```sh
make install
```

**Deploy the Manager to the cluster with the image specified by `IMG`:**

```sh
make deploy IMG=<some-registry>/operator:tag
```

> **NOTE**: If you encounter RBAC errors, you may need to grant yourself cluster-admin
> privileges or be logged in as admin.

### To Uninstall

**Delete the APIs(CRDs) from the cluster:**

```sh
make uninstall
```

**UnDeploy the controller from the cluster:**

```sh
make undeploy
```

Or ...... delete the kind cluster.

```sh
kind delete cluster
```

## Project Distribution

Following are the steps to build the installer and distribute this project to users.

1. Build the installer for the image built and published in the registry:

```sh
make build-installer IMG=<some-registry>/operator:tag
```

NOTE: The makefile target mentioned above generates an 'install.yaml'
file in the dist directory. This file contains all the resources built
with Kustomize, which are necessary to install this project without
its dependencies.

2. Using the installer

Users can just run kubectl apply -f <URL for YAML BUNDLE> to install the project, i.e.:

```sh
kubectl apply -f https://raw.githubusercontent.com/<org>/operator/<tag or branch>/dist/install.yaml
```

# FAQ

## How to use Private Registry Image Pulls?

Create a secret named `ghcr`:

```sh
kubectl create secret docker-registry ghcr --docker-server=https://ghcr.io --docker-username=<your-user> --docker-password=<your-token> --docker-email=<your-email>
```

then use it as imagePullSecret.

## What permissions does the Operator need on Kubernetes Core Resources?

Full RBAC definition [can be found here](./config/rbac/role.yaml)

### CRUD Cluster Wide ( Create Update Delete on any deployable namespace )

- apps
  - Deployment
  - Statefulset
- core
  - Service
  - ServiceAccount
  - ConfigMap
  - Secret
  - PersistentVolumeClaim
  - Event

### CRUD Operator namespace ( Create Update Delete specifically on `wasmcloud-system` )

- coordination.k8s.io
  - Lease

### Create Only

- authentication.k8s.io
  - SubjectAccessReview
