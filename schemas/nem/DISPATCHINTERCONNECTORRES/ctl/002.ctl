-- ************************************************************
-- Title:	PUBLIC_DVD_DISPATCHINTERCONNECTORRES_201907.ctl
-- ************************************************************
-- Project : NEMMCO MONTHLY DVD RELEASE
-- Purpose : Control File to load monthly csv file
-- Target Application  : Monthly DVD Release
-- Application Version : 1
-- Visibility Id : PUBLIC
--
-- ************************************************************
-- Special Instructions : 1. USER TO CONNECT AS DISPATCHINTERCONNECTORRES owner
--
-- ************************************************************
-- History :
--      MMS Data Model Version   Template Generated On
--      ======================   ======================
--      PUBLIC			       v
-- ************************************************************
LOAD DATA
INFILE PUBLIC_DVD_DISPATCHINTERCONNECTORRES_201907010000.CSV
APPEND INTO TABLE DISPATCHINTERCONNECTORRES
WHEN (1:1) = 'D'
FIELDS TERMINATED BY ','
OPTIONALLY ENCLOSED BY '"'
TRAILING NULLCOLS
(row_type FILLER,
report_type FILLER,
report_subtype FILLER,
report_version FILLER,
SETTLEMENTDATE DATE "yyyy/mm/dd hh24:mi:ss",
RUNNO FLOAT EXTERNAL,
INTERCONNECTORID CHAR(10),
DISPATCHINTERVAL FLOAT EXTERNAL,
INTERVENTION FLOAT EXTERNAL,
METEREDMWFLOW FLOAT EXTERNAL,
MWFLOW FLOAT EXTERNAL,
MWLOSSES FLOAT EXTERNAL,
MARGINALVALUE FLOAT EXTERNAL,
VIOLATIONDEGREE FLOAT EXTERNAL,
LASTCHANGED DATE "yyyy/mm/dd hh24:mi:ss",
EXPORTLIMIT FLOAT EXTERNAL,
IMPORTLIMIT FLOAT EXTERNAL,
MARGINALLOSS FLOAT EXTERNAL,
EXPORTGENCONID CHAR(20),
IMPORTGENCONID CHAR(20),
FCASEXPORTLIMIT FLOAT EXTERNAL,
FCASIMPORTLIMIT FLOAT EXTERNAL)
-- End of Script
