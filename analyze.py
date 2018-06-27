#!/usr/bin/env python

from plumbum.cmd import circo, gv2gml, xdg_open
from networkit import graphio, Format, profiling, centrality
import matplotlib.pyplot as plt

if __name__ == "__main__":
    # convert output to viewable postscript
    circo["-Tps", "graph/graph.dot", "-o", "graph/graph.ps"]()

    # networkit can't read dot, only write it, but it can read graphml, which petgraph can write with an extension
    graph = graphio.readGraph("graph/graph.graphml", fileformat=Format.GraphML)

    print("Nodes: {} Edges: {}".format(graph.numberOfNodes(), graph.numberOfEdges()))

    # this and other possibilities for calculating and plotting interesting stuff:
    # http://nbviewer.jupyter.org/urls/networkit.iti.kit.edu/uploads/docs/NetworKit_UserGuide.ipynb
    pf = profiling.Profile.create(graph, preset="minimal")
    pf.output("HTML", ".")
    xdg_open["graph/graph.html"]()

    dd = sorted(centrality.DegreeCentrality(graph).run().scores(), reverse=True)
    plt.xscale("log")
    plt.xlabel("degree")
    plt.yscale("log")
    plt.ylabel("number of nodes")
    plt.plot(dd)
    plt.show()
