-- ************************************************************
-- Title:	PUBLIC_ARCHIVE#P5MIN_CONSTRAINTSOLUTION#FILE02#202608.ctl
-- ************************************************************
-- Project : NEMMCO MONTHLY DVD RELEASE
-- Purpose : Control File to load monthly csv file
-- Target Application  : Monthly DVD Release
-- Application Version : 1
-- Visibility Id : PUBLIC
--
-- ************************************************************
-- Special Instructions : 1. USER TO CONNECT AS P5MIN_CONSTRAINTSOLUTION owner
--
-- ************************************************************
-- History :
--      MMS Data Model Version   Template Generated On
--      ======================   ======================
--      PUBLIC			       v
-- ************************************************************
LOAD DATA
INFILE PUBLIC_ARCHIVE#P5MIN_CONSTRAINTSOLUTION#FILE02#202608010000.CSV
APPEND INTO TABLE P5MIN_CONSTRAINTSOLUTION
WHEN (1:1) = 'D'
FIELDS TERMINATED BY ','
OPTIONALLY ENCLOSED BY '"'
TRAILING NULLCOLS
(row_type FILLER,
report_type FILLER,
report_subtype FILLER,
report_version FILLER,
RUN_DATETIME DATE "YYYY/MM/DD HH24:MI:SS",
INTERVAL_DATETIME DATE "YYYY/MM/DD HH24:MI:SS",
CONSTRAINTID CHAR(20),
RHS FLOAT EXTERNAL,
MARGINALVALUE FLOAT EXTERNAL,
VIOLATIONDEGREE FLOAT EXTERNAL,
LASTCHANGED DATE "YYYY/MM/DD HH24:MI:SS",
DUID CHAR(20),
GENCONID_EFFECTIVEDATE DATE "YYYY/MM/DD HH24:MI:SS",
GENCONID_VERSIONNO FLOAT EXTERNAL,
LHS FLOAT EXTERNAL,
INTERVENTION FLOAT EXTERNAL)
-- End of Script
