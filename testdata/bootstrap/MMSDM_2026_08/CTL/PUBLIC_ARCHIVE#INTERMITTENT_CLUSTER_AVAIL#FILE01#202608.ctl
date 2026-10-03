-- ************************************************************
-- Title:	PUBLIC_ARCHIVE#INTERMITTENT_CLUSTER_AVAIL#FILE01#202608.ctl
-- ************************************************************
-- Project : NEMMCO MONTHLY DVD RELEASE
-- Purpose : Control File to load monthly csv file
-- Target Application  : Monthly DVD Release
-- Application Version : 1
-- Visibility Id : PUBLIC
--
-- ************************************************************
-- Special Instructions : 1. USER TO CONNECT AS INTERMITTENT_CLUSTER_AVAIL owner
--
-- ************************************************************
-- History :
--      MMS Data Model Version   Template Generated On
--      ======================   ======================
--      PUBLIC			       v
-- ************************************************************
LOAD DATA
INFILE PUBLIC_ARCHIVE#INTERMITTENT_CLUSTER_AVAIL#FILE01#202608010000.CSV
APPEND INTO TABLE INTERMITTENT_CLUSTER_AVAIL
WHEN (1:1) = 'D'
FIELDS TERMINATED BY ','
OPTIONALLY ENCLOSED BY '"'
TRAILING NULLCOLS
(row_type FILLER,
report_type FILLER,
report_subtype FILLER,
report_version FILLER,
TRADINGDATE DATE "YYYY/MM/DD HH24:MI:SS",
DUID CHAR(20),
OFFERDATETIME DATE "YYYY/MM/DD HH24:MI:SS",
CLUSTERID CHAR(20),
PERIODID FLOAT EXTERNAL,
ELEMENTS_UNAVAILABLE FLOAT EXTERNAL,
ELEMENTS_AVAILABLE FLOAT EXTERNAL)
-- End of Script
